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
    outcome::{after_volume, clear_journal, interrupted_note, journal_note, retained_note},
};
use crate::{
    backend::Backend,
    cancel::{Cancellation, Signal},
    cleanup::CleanupFailure,
    config_store::ConfigWriter,
    ownership::{Disposition, LedgerError, ResourceKind, ResourceLedger, ResourceRef},
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
        let InitContext { ledger, .. } = *context;
        Self::promote_retained(ledger, volume)?;
        let warnings = self.cleanup_retained(context, volume).await?;
        self.write_retained(context, volume, warnings)
    }

    /// Marks the created volume as retained, refusing to continue unless the
    /// ledger and journal agree.
    fn promote_retained(
        ledger: &ResourceLedger,
        volume: &VolumeHandle,
    ) -> Result<(), InitError<B::Error>> {
        // From here the volume is the user's cache: it must survive cleanup.
        // The promotion must reach the journal before cleanup can delete an
        // `Owned` volume, so a persistence failure stops here rather than
        // risking a later recovery deleting the formatted cache.
        match ledger.promote(
            ResourceKind::InstanceVolume,
            &volume.id,
            Disposition::Retained,
        ) {
            Ok(()) => {}
            Err(error @ LedgerError::Persist { .. }) => {
                return Err(after_volume(
                    &InitError::Ledger(error.to_string()),
                    volume,
                    format!(
                        "{retained}; the ledger could not record the retention ({error})",
                        retained = retained_note(&volume.id)
                    ),
                    None,
                ));
            }
            Err(LedgerError::NotRecorded { .. }) => ledger.record(
                ResourceRef::instance_volume(&volume.id, &volume.zone),
                Disposition::Retained,
            ),
        }
        if ledger
            .find(ResourceKind::InstanceVolume, &volume.id)
            .is_some_and(|entry| entry.disposition == Disposition::Owned)
        {
            // A failed record() leaves the previous `Owned` entry in place; the
            // journal must not be trusted to preserve a volume it still marks
            // for deletion.
            return Err(after_volume(
                &InitError::Cleanup(CleanupFailure::new()),
                volume,
                format!(
                    "{retained}; the ledger still records the volume as owned",
                    retained = retained_note(&volume.id)
                ),
                None,
            ));
        }
        Ok(())
    }

    /// Runs the ownership-aware cleanup and keeps the warnings it reports.
    async fn cleanup_retained(
        &self,
        context: &InitContext<'_>,
        volume: &VolumeHandle,
    ) -> Result<Vec<String>, InitError<B::Error>> {
        let mut warnings = Vec::new();
        match self.cleanup(context.ledger, context.cancellation).await {
            CleanupOutcome::Completed(report) => warnings.extend(report.warnings),
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
                    interrupted_note(context.ledger),
                    None,
                ));
            }
        }
        Ok(warnings)
    }

    /// Reports a signal that arrived while cleanup ran; the volume is retained,
    /// so its identifier is still written first.
    fn cancelled_after_cleanup(
        &self,
        context: &InitContext<'_>,
        volume: &VolumeHandle,
        signal: Signal,
    ) -> InitError<B::Error> {
        let InitContext {
            request, ledger, ..
        } = *context;
        let written = self
            .config_writer
            .write_volume_id(&volume.id, request.overwrite_existing_volume_id);
        let journal = match &written {
            Ok(_) => {
                if ledger.clear_journal().is_ok() {
                    None
                } else {
                    ledger.journal_path()
                }
            }
            Err(_) => ledger.journal_path(),
        };
        InitError::Cancelled {
            message: format!(
                "{} received during cleanup; the volume {volume_id} is retained{written}",
                signal.as_str(),
                volume_id = volume.id,
                written = match written {
                    Ok(path) => format!(" and recorded in {path}"),
                    Err(error) => format!(", but recording it failed: {error}"),
                }
            ),
            signal,
            cleanup: None,
            journal,
        }
    }

    /// Writes the retained volume identifier unless a signal interrupted the
    /// initialization during cleanup.
    fn write_retained(
        &self,
        context: &InitContext<'_>,
        volume: &VolumeHandle,
        warnings: Vec<String>,
    ) -> Result<InitOutcome, InitError<B::Error>> {
        let InitContext {
            request,
            ledger,
            cancellation,
        } = *context;
        if let Some(signal) = cancellation.signal() {
            return Err(self.cancelled_after_cleanup(context, volume, signal));
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
                    warnings,
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
            CleanupOutcome::Completed(_) => {
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
            CleanupOutcome::Completed(_) => {
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
        let mut journal = ledger.journal_path();
        let mut cleanup_failure = None;
        let mut warnings = Vec::new();
        let state = match self.cleanup(ledger, cancellation).await {
            CleanupOutcome::Completed(report) => {
                warnings.extend(report.warnings);
                // Owned resources are gone, so a journal with nothing left to
                // recover must not keep warning the operator.
                journal = clear_journal(ledger, &mut warnings);
                String::from("provisioned resources were cleaned up")
            }
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
                Ok(report) => CleanupOutcome::Completed(report),
                Err(error) => CleanupOutcome::Failed(error),
            },
            () = cancellation.repeated() => CleanupOutcome::Interrupted,
        }
    }
}
