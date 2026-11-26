//! Volume initialization orchestration for `mriya init`.

use std::{ffi::OsString, fmt::Display};

use camino::Utf8PathBuf;
use helpers::{volume_name_for_project, volume_size_bytes};
use ortho_config::OrthoConfig;
use serde::Deserialize;

use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest},
    cache_device::{self, CacheDeviceReport, CacheDeviceStatus},
    config::ScalewayConfig,
    config_store::{ConfigStoreError, ConfigWriter},
    sync::{CommandRunner, SyncError, Syncer},
    volume::{
        MIN_VOLUME_SIZE_GB,
        VolumeBackend,
        VolumeHandle,
        VolumeRequest,
        VolumeStatus,
        VolumeTier,
    },
};

mod error;

mod helpers;

pub use error::{InitConfigError, InitError, InitRequestError};

/// Init-specific configuration values layered via `OrthoConfig`.
#[derive(Clone, Debug, Deserialize, OrthoConfig, PartialEq, Eq)]
#[ortho_config(
    prefix = "MRIYA_INIT",
    discovery(
        app_name = "mriya",
        env_var = "MRIYA_CONFIG_PATH",
        config_file_name = "mriya.toml",
        dotfile_name = ".mriya.toml",
        project_file_name = "mriya.toml"
    )
)]
pub struct InitConfig {
    /// Size of the cache volume in decimal gigabytes (10^9 bytes).
    #[ortho_config(default = 20)]
    pub volume_size_gb: u32,
    /// Block Storage performance tier: `sbs_5k` (default) or `sbs_15k`.
    #[ortho_config(default = "sbs_5k".to_owned())]
    pub volume_tier: String,
}

impl InitConfig {
    /// Loads init configuration without parsing CLI arguments.
    ///
    /// # Errors
    ///
    /// Returns [`InitConfigError::Parse`] when merging sources fails.
    pub fn load_without_cli_args() -> Result<Self, InitConfigError> {
        Self::load_from_iter([OsString::from("mriya")])
            .map_err(|err| InitConfigError::Parse(err.to_string()))
    }

    /// Validates init configuration.
    ///
    /// # Errors
    ///
    /// Returns [`InitConfigError::InvalidVolumeSize`] when the configured
    /// volume size is below the provider's 5 GB minimum, and
    /// [`InitConfigError::InvalidVolumeTier`] when the tier is not `sbs_5k` or
    /// `sbs_15k`.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::InitConfig;
    ///
    /// let config = InitConfig {
    ///     volume_size_gb: 4,
    ///     volume_tier: String::from("sbs_5k"),
    /// };
    /// assert!(config.validate().is_err());
    ///
    /// let config = InitConfig {
    ///     volume_size_gb: 5,
    ///     volume_tier: String::from("sbs_15k"),
    /// };
    /// assert!(config.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), InitConfigError> {
        if u64::from(self.volume_size_gb) < MIN_VOLUME_SIZE_GB {
            return Err(InitConfigError::InvalidVolumeSize);
        }
        VolumeTier::parse(&self.volume_tier)?;
        Ok(())
    }

    /// Returns the parsed performance tier.
    ///
    /// # Errors
    ///
    /// Returns [`InitConfigError::InvalidVolumeTier`] for an unsupported value.
    pub fn tier(&self) -> Result<VolumeTier, InitConfigError> {
        Ok(VolumeTier::parse(&self.volume_tier)?)
    }
}

/// Inputs required to prepare a cache volume.
#[derive(Clone, Debug)]
pub struct InitRequest {
    /// Volume creation parameters.
    pub volume: VolumeRequest,
    /// Instance request used to format the volume.
    pub instance_request: InstanceRequest,
    /// Whether to overwrite any existing configured volume ID.
    pub overwrite_existing_volume_id: bool,
    /// Effective merged volume ID at request time, including the environment.
    ///
    /// The file may hold a different value; this is what `mriya run` will
    /// actually use, so init must refuse to create a cache that would be
    /// ignored.
    pub effective_volume_id: Option<String>,
}

impl InitRequest {
    /// Builds an init request from configuration and the project name.
    ///
    /// # Errors
    ///
    /// Returns [`InitRequestError`] when configuration or validation fails.
    pub fn from_config(
        scaleway: &ScalewayConfig,
        init_config: &InitConfig,
        project_name: &str,
        overwrite_existing_volume_id: bool,
    ) -> Result<Self, InitRequestError> {
        init_config.validate()?;
        let tier = init_config.tier()?;
        let volume_name = volume_name_for_project(project_name);
        if volume_name.trim().is_empty() {
            return Err(InitRequestError::InvalidVolumeName);
        }
        let size_bytes =
            volume_size_bytes(init_config.volume_size_gb).ok_or(InitRequestError::SizeOverflow)?;

        scaleway.validate()?;
        let instance_request = InstanceRequest::builder()
            .image_label(&scaleway.default_image)
            .instance_type(&scaleway.default_instance_type)
            .zone(&scaleway.default_zone)
            .project_id(&scaleway.default_project_id)
            .organisation_id(scaleway.default_organization_id.clone())
            .architecture(&scaleway.default_architecture)
            .cloud_init_user_data(None)
            .build()
            .map_err(|err| InitRequestError::RequestBuild(err.to_string()))?;

        let effective_volume_id = scaleway
            .default_volume_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);

        let volume = VolumeRequest::new(
            volume_name,
            size_bytes,
            scaleway.default_zone.clone(),
            scaleway.default_project_id.clone(),
        )
        .organisation_id(scaleway.default_organization_id.clone())
        .with_tier(tier);

        Ok(Self {
            volume,
            instance_request,
            overwrite_existing_volume_id,
            effective_volume_id,
        })
    }
}

/// Outcome returned after successfully preparing a cache volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitOutcome {
    /// Identifier of the newly created volume.
    pub volume_id: String,
    /// Configuration file path that was updated.
    pub config_path: Utf8PathBuf,
    /// Warning to show the user, such as an environment ID that will override
    /// the newly written file value.
    pub warning: Option<String>,
}

/// Coordinates volume creation, formatting, and configuration updates.
#[derive(Debug)]
pub struct InitOrchestrator<B, R: CommandRunner, W> {
    /// Backend that creates the volume and manages the helper instance.
    ///
    /// The orchestrator owns exactly the volume it created. A later failure
    /// tears the formatter down and deletes that volume, never a previously
    /// configured ID. If deletion fails, the error names the volume and the
    /// `scw block volume delete` command for manual recovery.
    backend: B,
    /// Runs the format command on the helper instance over SSH.
    syncer: Syncer<R>,
    /// Records the new volume ID in the configuration file.
    config_writer: W,
}

impl<B, R, W> InitOrchestrator<B, R, W>
where
    B: Backend + VolumeBackend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
    W: ConfigWriter,
{
    /// Creates a new init orchestrator.
    #[must_use]
    pub const fn new(backend: B, syncer: Syncer<R>, config_writer: W) -> Self {
        Self {
            backend,
            syncer,
            config_writer,
        }
    }

    /// Executes the cache volume preparation workflow.
    ///
    /// # Errors
    ///
    /// Returns [`InitError`] when volume creation, formatting, teardown, or
    /// configuration updates fail.
    pub async fn execute(&self, request: &InitRequest) -> Result<InitOutcome, InitError<B::Error>> {
        let file_volume_id = self.config_writer.current_volume_id()?;
        self.ensure_configurable(request, file_volume_id.as_deref())?;

        let volume = self
            .backend
            .create_volume(&request.volume)
            .await
            .map_err(InitError::Volume)?;

        if let Err(source) = self
            .backend
            .wait_for_volume_status(&volume.zone, &volume.id, VolumeStatus::Available)
            .await
        {
            let message = source.to_string();
            let error = InitError::VolumeReady { message, source };
            return Err(self.rollback(&volume, None, error).await);
        }

        let mut instance_request = request.instance_request.clone();
        instance_request.volume_id = Some(volume.id.clone());
        instance_request.cloud_init_user_data = None;

        let handle = match self.backend.create(&instance_request).await {
            Ok(handle) => handle,
            Err(source) => {
                let error = InitError::Provision(source);
                return Err(self.rollback(&volume, None, error).await);
            }
        };

        let networking = match self.backend.wait_for_ready(&handle).await {
            Ok(networking) => networking,
            Err(source) => {
                let message = source.to_string();
                let error = InitError::Wait { message, source };
                return Err(self.rollback(&volume, Some(&handle), error).await);
            }
        };

        if let Err(failure) = self.format_volume(&networking, &volume.id) {
            let error = format_error(&volume.id, failure);
            return Err(self.rollback(&volume, Some(&handle), error).await);
        }

        if let Err(source) = self.backend.teardown(&handle, Some(&volume.id)).await {
            return Err(InitError::Teardown(source));
        }

        let config_path = match self
            .config_writer
            .write_volume_id(&volume.id, request.overwrite_existing_volume_id)
        {
            Ok(path) => path,
            Err(error) => {
                return Err(self.rollback(&volume, None, InitError::Config(error)).await);
            }
        };

        let warning = request
            .effective_volume_id
            .as_deref()
            .filter(|effective| Some(*effective) != file_volume_id.as_deref())
            .map(|effective| {
                format!(
                    "SCW_DEFAULT_VOLUME_ID={effective} still overrides the new default_volume_id={}; unset it to use the new cache volume",
                    volume.id
                )
            });

        Ok(InitOutcome {
            volume_id: volume.id,
            config_path,
            warning,
        })
    }

    /// Fails before creating anything when an effective volume ID is already
    /// configured and overwriting was not requested.
    ///
    /// The effective value includes `SCW_DEFAULT_VOLUME_ID`, which `mriya run`
    /// prefers over the file, so creating a new volume while it is set would
    /// silently leave the new cache unused.
    fn ensure_configurable(
        &self,
        request: &InitRequest,
        file_volume_id: Option<&str>,
    ) -> Result<(), InitError<B::Error>> {
        if request.overwrite_existing_volume_id {
            return Ok(());
        }

        if let Some(existing) = file_volume_id {
            return Err(InitError::Config(
                ConfigStoreError::VolumeAlreadyConfigured {
                    volume_id: existing.to_owned(),
                },
            ));
        }

        if let Some(effective) = request
            .effective_volume_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return Err(InitError::Config(
                ConfigStoreError::EffectiveVolumeConfigured {
                    volume_id: effective.to_owned(),
                },
            ));
        }

        Ok(())
    }

    /// Removes the owned volume and any formatter server after a failure.
    ///
    /// Only the handle created by this call is touched. A cleanup failure
    /// wraps the original error and names the volume for manual recovery.
    async fn rollback(
        &self,
        volume: &VolumeHandle,
        handle: Option<&InstanceHandle>,
        original: InitError<B::Error>,
    ) -> InitError<B::Error> {
        let mut cleanup = Vec::new();

        if let Some(handle) = handle
            && let Err(error) = self.backend.teardown(handle, Some(&volume.id)).await
        {
            cleanup.push(format!(
                "tearing down formatter {} failed: {error}",
                handle.id
            ));
        }

        if let Err(error) = self.backend.delete_volume(volume).await {
            cleanup.push(format!(
                "deleting volume {} in zone {} failed: {error}; delete it with `scw block volume delete {} zone={}`",
                volume.id, volume.zone, volume.id, volume.zone
            ));
        }

        if cleanup.is_empty() {
            original
        } else {
            let original: Box<dyn std::error::Error + Send + Sync + 'static> = Box::new(original);
            InitError::Rollback {
                original,
                cleanup: cleanup.join("; "),
            }
        }
    }

    /// Formats the new volume through the guarded cache-device script.
    ///
    /// The script identifies the device by its `volume-<uuid>` serial, refuses
    /// the root disk and any ancestor, refuses a device that already carries a
    /// filesystem signature, labels the new ext4 filesystem and flushes it.
    /// The single status line is parsed rather than trusting the exit code.
    fn format_volume(
        &self,
        networking: &InstanceNetworking,
        volume_id: &str,
    ) -> Result<(), FormatFailure> {
        let script = cache_device::format_script(volume_id);
        let output = self
            .syncer
            .run_remote_raw(networking, &script)
            .map_err(format_spawn_failure)?;

        let report = cache_device::parse_report(&output.stdout).map_err(|error| {
            FormatFailure::Protocol {
                message: format!("format script produced no usable status: {error}"),
            }
        })?;

        if report.status().is_success() {
            return Ok(());
        }
        Err(FormatFailure::Report(report))
    }
}

/// Builds the failure for a format command that could not be started.
///
/// `write_error` prints only the outermost `Display` and no cause chain, so the
/// cause and its remedy have to be in the message itself. A stored source
/// alone would report "failed to execute format command", and a user whose
/// `HOME` cannot be decoded would look for a broken volume instead.
fn format_spawn_failure(err: SyncError) -> FormatFailure {
    FormatFailure::Spawn {
        message: format!("failed to execute format command: {err}"),
        source: err,
    }
}

/// Why formatting failed, kept so the error can carry its cause.
#[derive(Debug)]
enum FormatFailure {
    /// The SSH command could not be started.
    Spawn {
        /// Description that names the underlying cause and remedy.
        message: String,
        /// Underlying SSH error.
        source: SyncError,
    },
    /// The script ran but printed no usable status line.
    Protocol {
        /// Description of the missing status.
        message: String,
    },
    /// The script reported a refusal.
    Report(CacheDeviceReport),
}

/// Maps a formatting failure to the matching typed init error.
fn format_error<BackendError: std::error::Error + 'static>(
    volume_id: &str,
    failure: FormatFailure,
) -> InitError<BackendError> {
    match failure {
        FormatFailure::Spawn { message, source } => InitError::FormatFailed {
            volume_id: volume_id.to_owned(),
            message,
            source: Some(source),
        },
        FormatFailure::Protocol { message } => InitError::FormatFailed {
            volume_id: volume_id.to_owned(),
            message,
            source: None,
        },
        FormatFailure::Report(report) => {
            let message = report.describe();
            match report.status() {
                CacheDeviceStatus::NotFound => InitError::FormatDeviceNotFound {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                CacheDeviceStatus::Ambiguous => InitError::FormatDeviceAmbiguous {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                CacheDeviceStatus::RootDisk => InitError::FormatRootDisk {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                CacheDeviceStatus::RootUnknown => InitError::FormatRootUnknown {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                CacheDeviceStatus::SignaturePresent => InitError::FormatSignaturePresent {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                _ => InitError::FormatFailed {
                    volume_id: volume_id.to_owned(),
                    message,
                    source: None,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests;
