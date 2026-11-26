//! Init orchestration: guarded creation, formatting and ownership-safe cleanup.

use std::fmt::Display;

use super::{
    error::InitError,
    request::{InitOutcome, InitRequest},
};
use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking},
    cache_device::{self, CacheDeviceReport, CacheDeviceStatus},
    config_store::{ConfigStoreError, ConfigWriter},
    sync::{CommandRunner, SyncError, Syncer},
    volume::{VolumeBackend, VolumeHandle, VolumeStatus},
};

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
        Self::ensure_configurable(request, file_volume_id.as_deref())?;

        let volume = self.create_ready_volume(request).await?;

        let mut instance_request = request.instance_request.clone();
        instance_request.volume_id = Some(volume.id.clone());
        instance_request.cloud_init_user_data = None;

        let handle = match self.backend.create(&instance_request).await {
            Ok(handle) => handle,
            Err(source) => {
                let error = InitError::Provision(Box::new(source));
                return Err(self.rollback(&volume, None, error).await);
            }
        };

        let networking = match self.backend.wait_for_ready(&handle).await {
            Ok(networking) => networking,
            Err(source) => {
                let message = source.to_string();
                let error = InitError::Wait {
                    message,
                    source: Box::new(source),
                };
                return Err(self.rollback(&volume, Some(&handle), error).await);
            }
        };

        if let Err(failure) = self.format_volume(&networking, &volume.id) {
            let error = format_error(&volume.id, failure);
            return Err(self.rollback(&volume, Some(&handle), error).await);
        }

        if let Err(source) = self.backend.teardown(&handle, Some(&volume.id)).await {
            return Err(InitError::Teardown(Box::new(source)));
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

    /// Creates the cache volume and waits for its documented ready state.
    ///
    /// A failure after creation rolls the owned volume back, so a volume that
    /// never becomes available does not outlive the command that made it.
    async fn create_ready_volume(
        &self,
        request: &InitRequest,
    ) -> Result<VolumeHandle, InitError<B::Error>> {
        let volume = self
            .backend
            .create_volume(&request.volume)
            .await
            .map_err(|source| InitError::Volume(Box::new(source)))?;

        if let Err(source) = self
            .backend
            .wait_for_volume_status(&volume.zone, &volume.id, VolumeStatus::Available)
            .await
        {
            let message = source.to_string();
            let error = InitError::VolumeReady {
                message,
                source: Box::new(source),
            };
            return Err(self.rollback(&volume, None, error).await);
        }

        Ok(volume)
    }

    /// Fails before creating anything when an effective volume ID is already
    /// configured and overwriting was not requested.
    ///
    /// The effective value includes `SCW_DEFAULT_VOLUME_ID`, which `mriya run`
    /// prefers over the file, so creating a new volume while it is set would
    /// silently leave the new cache unused.
    fn ensure_configurable(
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

        if let Some(formatter) = handle
            && let Err(error) = self.backend.teardown(formatter, Some(&volume.id)).await
        {
            cleanup.push(format!(
                "tearing down formatter {} failed: {error}",
                formatter.id
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
            InitError::Rollback {
                original: Box::new(original),
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
pub(crate) fn format_spawn_failure(err: SyncError) -> FormatFailure {
    FormatFailure::Spawn {
        message: format!("failed to execute format command: {err}"),
        source: err,
    }
}

/// Why formatting failed, kept so the error can carry its cause.
#[derive(Debug)]
pub(crate) enum FormatFailure {
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
pub(crate) fn format_error<BackendError: std::error::Error + 'static>(
    volume_id: &str,
    failure: FormatFailure,
) -> InitError<BackendError> {
    match failure {
        FormatFailure::Spawn { message, source } => InitError::FormatFailed {
            volume_id: volume_id.to_owned(),
            message,
            source: Some(Box::new(source)),
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
