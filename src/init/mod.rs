//! Volume initialization orchestration for `mriya init`.

use std::{ffi::OsString, fmt::Display};

use camino::Utf8PathBuf;
use helpers::{format_command, volume_name_for_project, volume_size_bytes};
use ortho_config::OrthoConfig;
use serde::Deserialize;

use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest},
    config_store::{ConfigStoreError, ConfigWriter},
    sync::{CommandRunner, RemoteCommandOutput, SyncError, Syncer},
    teardown::append_teardown_note,
    volume::{VolumeBackend, VolumeHandle, VolumeRequest},
};

mod error;

mod helpers;

pub use error::{InitConfigError, InitError, InitRequestError};

mod request;

pub use request::InitRequest;

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
    /// Size of the cache volume in gigabytes.
    #[ortho_config(default = 20)]
    pub volume_size_gb: u32,
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
    /// volume size is zero.
    pub const fn validate(&self) -> Result<(), InitConfigError> {
        if self.volume_size_gb == 0 {
            return Err(InitConfigError::InvalidVolumeSize);
        }
        Ok(())
    }
}

/// Outcome returned after successfully preparing a cache volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitOutcome {
    /// Identifier of the newly created volume.
    pub volume_id: String,
    /// Configuration file path that was updated.
    pub config_path: Utf8PathBuf,
}

/// Coordinates volume creation, formatting, and configuration updates.
#[derive(Debug)]
pub struct InitOrchestrator<B, R: CommandRunner, W> {
    /// Backend that creates the volume and destroys the helper instance.
    ///
    /// The volume is deliberately not destroyed on failure: no method bound
    /// here can delete one, so a failed run leaves it in place (issue #154).
    /// Janitor cleanup is not a safety net for it in general: the janitor
    /// sweeps only volumes carrying an `mriya-test-run-<id>` tag, and this
    /// volume carries one only when `MRIYA_TEST_RUN_ID` holds a non-blank
    /// value. Without that variable the janitor will not remove a leaked
    /// volume, and it remains until an operator deletes it manually.
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
        self.ensure_configurable(request)?;

        let volume = self
            .backend
            .create_volume(&request.volume)
            .await
            .map_err(InitError::Volume)?;

        let (handle, networking) = self.prepare_instance(request, &volume).await?;
        let format_result = self.format_volume(&networking, &volume.id);
        self.handle_failure_or_destroy(&handle, format_result, |message, failure| {
            InitError::Format {
                message,
                source: failure.source,
            }
        })
        .await?;

        let detach_result = self.backend.detach_volume(&handle, &volume.id).await;
        self.handle_failure_or_destroy(&handle, detach_result, |message, err| InitError::Detach {
            message,
            source: err,
        })
        .await?;

        self.backend
            .destroy(handle)
            .await
            .map_err(InitError::Teardown)?;

        let config_path = self
            .config_writer
            .write_volume_id(&volume.id, request.overwrite_existing_volume_id)?;

        Ok(InitOutcome {
            volume_id: volume.id,
            config_path,
        })
    }

    /// Fails before creating anything when a volume ID is already configured
    /// and overwriting was not requested.
    ///
    /// A forced run may replace an ID the file holds, because the write changes
    /// what `run` selects. An ID the environment supplies is different: the
    /// file write cannot change it, so the new volume would never be the
    /// effective one. That case fails even with `--force`.
    fn ensure_configurable(&self, request: &InitRequest) -> Result<(), InitError<B::Error>> {
        let stored = self.config_writer.current_volume_id()?;

        if let Some(configured) = request.configured_volume_id.as_deref()
            && stored.as_deref() != Some(configured)
        {
            return Err(InitError::EnvironmentVolumeId {
                volume_id: configured.to_owned(),
            });
        }

        if let Some(existing) = stored
            && !request.overwrite_existing_volume_id
        {
            return Err(InitError::Config(
                ConfigStoreError::VolumeAlreadyConfigured {
                    volume_id: existing,
                },
            ));
        }
        Ok(())
    }

    /// Creates a helper instance with the new volume attached and waits for it
    /// to be reachable.
    ///
    /// Cloud-init user-data is cleared so the helper boots with defaults.
    async fn prepare_instance(
        &self,
        request: &InitRequest,
        volume: &VolumeHandle,
    ) -> Result<(InstanceHandle, InstanceNetworking), InitError<B::Error>> {
        let mut instance_request = request.instance_request.clone();
        instance_request.volume_id = Some(volume.id.clone());
        instance_request.cloud_init_user_data = None;

        let handle = self
            .backend
            .create(&instance_request)
            .await
            .map_err(InitError::Provision)?;

        let networking = self.wait_for_ready_or_destroy(&handle).await?;
        Ok((handle, networking))
    }

    /// Waits for SSH readiness, destroying the instance if that fails.
    async fn wait_for_ready_or_destroy(
        &self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, InitError<B::Error>> {
        match self.backend.wait_for_ready(handle).await {
            Ok(net) => Ok(net),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(InitError::Wait {
                    message,
                    source: err,
                })
            }
        }
    }

    /// Passes `Ok` through. On failure, destroys the instance and builds the
    /// error with `make_error` from the message and original error.
    async fn handle_failure_or_destroy<E>(
        &self,
        handle: &InstanceHandle,
        result: Result<(), E>,
        make_error: impl FnOnce(String, E) -> InitError<B::Error>,
    ) -> Result<(), InitError<B::Error>>
    where
        E: Display,
    {
        match result {
            Ok(()) => Ok(()),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(make_error(message, err))
            }
        }
    }

    /// Formats the volume on the helper instance, failing when the command
    /// cannot be run or exits with anything other than 0.
    fn format_volume(
        &self,
        networking: &InstanceNetworking,
        volume_id: &str,
    ) -> Result<(), FormatFailure> {
        let output = self
            .syncer
            .run_remote_raw(networking, &format_command(volume_id))
            .map_err(format_spawn_failure)?;

        if output.exit_code == Some(0) {
            return Ok(());
        }

        Err(FormatFailure {
            message: format_failure_message(&output),
            source: None,
        })
    }

    /// Destroys the instance and returns `err` as text, with a note appended
    /// when the teardown also fails.
    async fn destroy_with_note<E: Display>(&self, handle: &InstanceHandle, err: &E) -> String {
        let teardown_error = self.backend.destroy(handle.clone()).await.err();
        append_teardown_note(err.to_string(), teardown_error.as_ref())
    }
}

/// Builds the failure for a format command that could not be started.
///
/// `write_error` prints only the outermost `Display` and no cause chain, so the
/// cause and its remedy have to be in the message itself. A stored source
/// alone would report "failed to execute format command", and a user whose
/// `HOME` cannot be decoded would look for a broken volume instead.
fn format_spawn_failure(err: SyncError) -> FormatFailure {
    FormatFailure {
        message: format!("failed to execute format command: {err}"),
        source: Some(err),
    }
}

/// Why formatting failed, kept so the error can carry its cause.
#[derive(Debug)]
struct FormatFailure {
    /// Description of the failure, shown to the user.
    message: String,
    /// Underlying SSH error, absent when the command ran but exited non-zero.
    source: Option<SyncError>,
}

impl Display for FormatFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Describes a non-zero or missing exit status from `mkfs.ext4`, including
/// trimmed stderr when there is any.
fn format_failure_message(output: &RemoteCommandOutput) -> String {
    let stderr = output.stderr.trim();
    match output.exit_code {
        Some(code) if stderr.is_empty() => format!("mkfs.ext4 exited with status {code}"),
        Some(code) => format!("mkfs.ext4 exited with status {code}: {stderr}"),
        None if stderr.is_empty() => String::from("mkfs.ext4 terminated without an exit status"),
        None => format!("mkfs.ext4 terminated without an exit status: {stderr}"),
    }
}

#[cfg(test)]
mod tests;
