//! Init orchestration: guarded creation, formatting and ownership-safe cleanup.

use std::fmt::Display;

use super::{
    error::InitError,
    format::{FormatFailure, format_error, format_spawn_failure},
    request::{InitOutcome, InitRequest},
};
use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking},
    cache_device,
    config_store::{ConfigStoreError, ConfigWriter},
    sync::{CommandRunner, Syncer},
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
    ///
    /// # Parameters
    ///
    /// - `backend`: creates, attaches and tears down the volume and formatter.
    /// - `syncer`: runs the format command on the formatter over SSH.
    /// - `config_writer`: reads and records the configured volume ID.
    ///
    /// # Returns
    ///
    /// An orchestrator that owns exactly the volume it creates.
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
    /// # Parameters
    ///
    /// - `request`: volume, instance and overwrite settings.
    ///
    /// # Returns
    ///
    /// The new volume ID, the configuration path and an optional warning when
    /// the environment still overrides the recorded value. Success means the
    /// formatter is gone and the new ID is written.
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

        self.teardown_formatter(&handle, &volume).await?;

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
            .environment_volume_id
            .as_deref()
            .filter(|effective| *effective != volume.id)
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

    /// Tears the formatter down and deletes the detached volume if needed.
    ///
    /// Only a destroy failure proves the cache was detached, so only that
    /// stage deletes the volume this run created. A stop or detach failure
    /// leaves the server and volume in an unknown state, so the error is
    /// reported without touching the volume.
    async fn teardown_formatter(
        &self,
        handle: &InstanceHandle,
        volume: &VolumeHandle,
    ) -> Result<(), InitError<B::Error>> {
        match self.backend.teardown(handle, Some(&volume.id)).await {
            Ok(()) => Ok(()),
            Err(failure) if failure.stage == crate::volume::TeardownStage::Destroying => {
                self.delete_detached_volume(volume, failure).await
            }
            Err(failure) => Err(InitError::FormatterTeardown {
                message: failure.to_string(),
                source: Box::new(failure.source),
            }),
        }
    }

    /// Deletes the owned volume after the formatter's destroy failed.
    ///
    /// The formatter is detached at this stage, so the volume can be removed.
    /// The destroy failure is reported either way; the manual recovery command
    /// appears only when this deletion also fails.
    async fn delete_detached_volume(
        &self,
        volume: &VolumeHandle,
        failure: crate::volume::TeardownFailure<B::Error>,
    ) -> Result<(), InitError<B::Error>> {
        let destroy_message = failure.source.to_string();
        Err(match self.backend.delete_volume(volume).await {
            Ok(()) => InitError::FormatterTeardown {
                message: format!(
                    "{destroy_message}; the detached cache volume {} was deleted",
                    volume.id
                ),
                source: Box::new(failure.source),
            },
            Err(error) => InitError::TeardownAfterDetach {
                volume_id: volume.id.clone(),
                zone: volume.zone.clone(),
                message: format!(
                    "{destroy_message}; deleting the detached volume {} in zone {} failed: {error}",
                    volume.id, volume.zone
                ),
                source: Box::new(failure.source),
            },
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
