//! The init orchestrator: create, format, promote, clean up, and record.

use std::fmt::Display;

use super::{
    InitError,
    InitOutcome,
    InitRequest,
    finish::InitContext,
    format::{format_failure_message, format_spawn_failure},
    helpers::format_command,
};
use crate::{
    backend::{Backend, InstanceNetworking},
    cancel::{Cancellation, until_cancelled},
    cleanup::{CleanupFailure, CleanupReport},
    config_store::{ConfigStoreError, ConfigWriter},
    ownership::ResourceLedger,
    sync::{CommandRunner, Syncer},
    volume::{VolumeBackend, VolumeHandle},
};

/// Coordinates volume creation, formatting, and configuration updates.
#[derive(Debug)]
pub struct InitOrchestrator<B, R: CommandRunner, W> {
    /// Backend that creates the volume and cleans up the allocation.
    pub(super) backend: B,
    /// Runs the format command on the helper instance over SSH.
    pub(super) syncer: Syncer<R>,
    /// Records the new volume ID in the configuration file.
    pub(super) config_writer: W,
}

/// Why the initialization steps stopped.
pub(super) enum StepFailure<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// A step failed; cleanup still runs.
    Error(InitError<BackendError>),
    /// A signal interrupted a step or arrived between steps.
    Cancelled,
}

/// How ownership-aware cleanup ended.
pub(super) enum CleanupOutcome {
    /// Cleanup verified every postcondition.
    Completed(CleanupReport),
    /// Cleanup ran but could not verify its postconditions.
    Failed(CleanupFailure),
    /// A second signal stopped cleanup before it finished.
    Interrupted,
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
    /// Returns [`InitError`] when volume creation, formatting, cleanup, or
    /// configuration updates fail, or when a signal interrupts the workflow.
    pub async fn execute(
        &self,
        request: &InitRequest,
        ledger: &ResourceLedger,
        cancellation: &Cancellation,
    ) -> Result<InitOutcome, InitError<B::Error>> {
        self.ensure_configurable(request)?;
        let context = InitContext {
            request,
            ledger,
            cancellation,
        };

        let volume = match until_cancelled(
            self.backend.create_volume(&request.volume, ledger),
            cancellation,
        )
        .await
        {
            None => return Err(self.finish_cancelled(&context).await),
            Some(Err(error)) => {
                return Err(self.finish_failure_without_volume(&context, error).await);
            }
            Some(Ok(volume)) => volume,
        };

        match self.prepare_and_format(&context, &volume).await {
            Ok(()) => self.finish_success(&context, &volume).await,
            Err(StepFailure::Cancelled) => Err(self.finish_cancelled(&context).await),
            Err(StepFailure::Error(error)) => Err(self
                .finish_failure_after_volume(&context, &volume, error)
                .await),
        }
    }

    /// Fails before creating anything when a volume ID is already configured
    /// and overwriting was not requested.
    fn ensure_configurable(&self, request: &InitRequest) -> Result<(), InitError<B::Error>> {
        if let Some(existing) = self.config_writer.current_volume_id()?
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

    /// Creates the helper instance, waits for readiness, and formats the
    /// volume.
    async fn prepare_and_format(
        &self,
        context: &InitContext<'_>,
        volume: &VolumeHandle,
    ) -> Result<(), StepFailure<B::Error>> {
        let InitContext {
            request,
            ledger,
            cancellation,
        } = *context;
        let mut instance_request = request.instance_request.clone();
        instance_request.volume_id = Some(volume.id.clone());
        instance_request.cloud_init_user_data = None;

        let handle =
            match until_cancelled(self.backend.create(&instance_request, ledger), cancellation)
                .await
            {
                None => return Err(StepFailure::Cancelled),
                Some(Err(error)) => return Err(StepFailure::Error(InitError::Provision(error))),
                Some(Ok(handle)) => handle,
            };

        let networking =
            match until_cancelled(self.backend.wait_for_ready(&handle), cancellation).await {
                None => return Err(StepFailure::Cancelled),
                Some(Err(error)) => {
                    return Err(StepFailure::Error(InitError::Wait {
                        message: error.to_string(),
                        source: error,
                    }));
                }
                Some(Ok(networking)) => networking,
            };

        if cancellation.is_cancelled() {
            return Err(StepFailure::Cancelled);
        }

        match self.format_volume(&networking, &volume.id) {
            Ok(()) => Ok(()),
            // A signal kills the format child, so a non-zero status is an
            // interruption rather than a formatting defect.
            Err(_) if cancellation.is_cancelled() => Err(StepFailure::Cancelled),
            Err(error) => Err(StepFailure::Error(error)),
        }
    }

    /// Formats the volume, failing when the command cannot be run or exits
    /// with anything other than 0.
    fn format_volume(
        &self,
        networking: &InstanceNetworking,
        volume_id: &str,
    ) -> Result<(), InitError<B::Error>> {
        let output = self
            .syncer
            .run_remote_raw(networking, &format_command(volume_id))
            .map_err(|err| {
                let failure = format_spawn_failure(err);
                InitError::Format {
                    message: failure.message,
                    source: failure.source,
                }
            })?;

        if output.exit_code == Some(0) {
            return Ok(());
        }

        Err(InitError::Format {
            message: format_failure_message(&output),
            source: None,
        })
    }
}
