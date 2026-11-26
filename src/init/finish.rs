//! Cleanup and outcome composition for the init orchestrator.
//!
//! Initialization has one cleanup path and three outcomes: the volume was
//! deleted before formatting, the volume was retained and its identifier is in
//! configuration, or the volume was retained and must be recovered by
//! identifier. Each outcome keeps the primary failure and reports what happened
//! to the volume.

use std::fmt::Display;

use super::{
    InitError,
    InitErrorKind,
    InitOrchestrator,
    InitOutcome,
    InitRequest,
    orchestrator::CleanupOutcome,
};
use crate::{
    backend::Backend,
    cancel::{Cancellation, Signal},
    cleanup::CleanupFailure,
    config_store::ConfigWriter,
    ownership::{Disposition, ResourceKind, ResourceLedger, ResourceRef},
    sync::CommandRunner,
    volume::{VolumeBackend, VolumeHandle},
};

/// Inputs shared by the init steps and their cleanup.
#[derive(Clone, Copy)]
pub(super) struct InitContext<'a> {
    /// The request that describes the volume to prepare.
    pub(super) request: &'a InitRequest,
    /// The run's ownership ledger.
    pub(super) ledger: &'a ResourceLedger,
    /// Cancellation view shared with the signal listener.
    pub(super) cancellation: &'a Cancellation,
}

/// Configuration key that holds the cache volume identifier.
const VOLUME_KEY: &str = "default_volume_id";
/// Configuration section that holds the cache volume identifier.
const SCALEWAY_SECTION: &str = "scaleway";
/// Environment variable that overrides the cache volume identifier.
const VOLUME_ENV_VAR: &str = "SCW_DEFAULT_VOLUME_ID";

impl<B, R, W> InitOrchestrator<B, R, W>
where
    B: Backend + VolumeBackend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
    W: ConfigWriter,
{
    /// Promotes the volume to retained, cleans up, and records the ID.
    pub(super) async fn finish_success(
        &self,
        context: &InitContext<'_>,
        volume: &VolumeHandle,
    ) -> Result<InitOutcome, InitError<B::Error>> {
        let InitContext {
            request,
            ledger,
            cancellation,
        } = *context;
        // From here the volume is the user's cache: it must survive cleanup.
        if ledger
            .promote(
                ResourceKind::InstanceVolume,
                &volume.id,
                Disposition::Retained,
            )
            .is_err()
        {
            ledger.record(
                ResourceRef::instance_volume(&volume.id, &volume.zone),
                Disposition::Retained,
            );
        }

        match self.cleanup(ledger, cancellation).await {
            CleanupOutcome::Completed => {}
            CleanupOutcome::Failed(failure) => {
                let note = format!(
                    "{retained}; cleanup could not verify it: {failure}",
                    retained = retained_note(&volume.id)
                );
                return Err(after_volume(
                    &InitError::Cleanup(failure.clone()),
                    volume,
                    note,
                    Some(failure),
                ));
            }
            CleanupOutcome::Interrupted => {
                return Err(after_volume(
                    &InitError::Cleanup(CleanupFailure::new()),
                    volume,
                    interrupted_note(ledger),
                    None,
                ));
            }
        }

        match self
            .config_writer
            .write_volume_id(&volume.id, request.overwrite_existing_volume_id)
        {
            Ok(config_path) => {
                ledger.clear_journal().ok();
                Ok(InitOutcome {
                    volume_id: volume.id.clone(),
                    config_path,
                })
            }
            Err(error) => Err(after_volume(
                &InitError::Config(error),
                volume,
                retained_note(&volume.id),
                None,
            )),
        }
    }

    /// Cleans up after a failure that happened after the volume was created.
    pub(super) async fn finish_failure_after_volume(
        &self,
        context: &InitContext<'_>,
        volume: &VolumeHandle,
        primary: InitError<B::Error>,
    ) -> InitError<B::Error> {
        let InitContext {
            ledger,
            cancellation,
            ..
        } = *context;
        match self.cleanup(ledger, cancellation).await {
            CleanupOutcome::Completed => {
                // The volume is gone and the formatter instance with it, so
                // the journal has nothing left to replay.
                ledger.clear_journal().ok();
                after_volume(
                    &primary,
                    volume,
                    format!(
                        "the new volume {volume_id} in zone {zone} was deleted",
                        volume_id = volume.id,
                        zone = volume.zone
                    ),
                    None,
                )
            }
            CleanupOutcome::Failed(failure) => {
                let note = format!(
                    "cleanup failed, so the new volume {volume_id} in zone {zone} may remain: \
                     {failure}",
                    volume_id = volume.id,
                    zone = volume.zone
                );
                after_volume(&primary, volume, note, Some(failure))
            }
            CleanupOutcome::Interrupted => {
                after_volume(&primary, volume, interrupted_note(ledger), None)
            }
        }
    }

    /// Cleans up after a failure before any volume identifier was recorded.
    pub(super) async fn finish_failure_without_volume(
        &self,
        context: &InitContext<'_>,
        primary: B::Error,
    ) -> InitError<B::Error> {
        let InitContext {
            ledger,
            cancellation,
            ..
        } = *context;
        match self.cleanup(ledger, cancellation).await {
            CleanupOutcome::Completed => {
                // Nothing was recorded as owned, so the journal is settled.
                ledger.clear_journal().ok();
                InitError::Volume(primary)
            }
            CleanupOutcome::Failed(failure) => {
                let kind = InitErrorKind::Volume;
                InitError::AfterVolume {
                    primary: InitError::<B::Error>::Volume(primary).to_string(),
                    kind,
                    volume_id: String::from("unknown"),
                    zone: ledger.scope().zone,
                    note: format!("cleanup failed and the allocation may remain: {failure}"),
                    cleanup: Some(Box::new(failure)),
                }
            }
            CleanupOutcome::Interrupted => {
                let kind = InitErrorKind::Volume;
                InitError::AfterVolume {
                    primary: InitError::<B::Error>::Volume(primary).to_string(),
                    kind,
                    volume_id: String::from("unknown"),
                    zone: ledger.scope().zone,
                    note: interrupted_note(ledger),
                    cleanup: None,
                }
            }
        }
    }

    /// Cleans up after a signal interrupted initialization.
    pub(super) async fn finish_cancelled(&self, context: &InitContext<'_>) -> InitError<B::Error> {
        let InitContext {
            ledger,
            cancellation,
            ..
        } = *context;
        let signal = cancellation.signal().unwrap_or(Signal::Interrupt);
        let journal = ledger.journal_path();
        let mut cleanup_failure = None;
        let state = match self.cleanup(ledger, cancellation).await {
            CleanupOutcome::Completed => String::from("provisioned resources were cleaned up"),
            CleanupOutcome::Failed(failure) => {
                let message = format!("ownership-aware cleanup did not complete: {failure}");
                cleanup_failure = Some(Box::new(failure));
                message
            }
            CleanupOutcome::Interrupted => {
                String::from("cleanup was interrupted; run `mriya recover` to finish it")
            }
        };
        InitError::Cancelled {
            message: format!(
                "{signal_name} received; {state}{journal_note}",
                signal_name = signal.as_str(),
                journal_note = journal_note(journal.as_deref())
            ),
            signal,
            cleanup: cleanup_failure,
            journal,
        }
    }

    /// Runs backend cleanup, aborting it when a second signal arrives.
    pub(super) async fn cleanup(
        &self,
        ledger: &ResourceLedger,
        cancellation: &Cancellation,
    ) -> CleanupOutcome {
        tokio::select! {
            result = self.backend.cleanup(ledger) => match result {
                Ok(_) => CleanupOutcome::Completed,
                Err(error) => CleanupOutcome::Failed(error),
            },
            () = cancellation.repeated() => CleanupOutcome::Interrupted,
        }
    }
}

/// Wraps `primary` in the volume context `note`.
fn after_volume<BackendError>(
    primary: &InitError<BackendError>,
    volume: &VolumeHandle,
    note: String,
    cleanup: Option<CleanupFailure>,
) -> InitError<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    let kind = primary.kind();
    InitError::AfterVolume {
        primary: primary.to_string(),
        kind,
        volume_id: volume.id.clone(),
        zone: volume.zone.clone(),
        note,
        cleanup: cleanup.map(Box::new),
    }
}

/// Describes how to configure a retained volume, including the recovery step.
pub(super) fn retained_note(volume_id: &str) -> String {
    format!(
        "the new volume {volume_id} is retained; configure it with \
         [{SCALEWAY_SECTION}] {VOLUME_KEY} = \"{volume_id}\" or {VOLUME_ENV_VAR}={volume_id}, \
         then run `mriya recover` to confirm it"
    )
}

/// Describes a cleanup interrupted by a second signal.
pub(super) fn interrupted_note(ledger: &ResourceLedger) -> String {
    format!(
        "cleanup was interrupted by a second signal; run `mriya recover`{note}",
        note = journal_note(ledger.journal_path().as_deref())
    )
}

/// Renders the journal path suffix for a message.
fn journal_note(journal: Option<&camino::Utf8Path>) -> String {
    journal.map_or_else(String::new, |path| format!(" (journal: {path})"))
}
