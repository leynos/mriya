//! Retaining the formatted volume and recording its identifier.
//!
//! Once formatting succeeds the volume belongs to the user: it is promoted to
//! `Retained` before cleanup runs, so cleanup detaches but never deletes it,
//! and its identifier is written to configuration. A promotion that cannot be
//! persisted still runs helper cleanup, but the journal is removed so recovery
//! cannot delete the formatted cache.

use std::fmt::Display;

use super::{
    InitError,
    InitOrchestrator,
    InitOutcome,
    finish::InitContext,
    orchestrator::CleanupOutcome,
    outcome::{after_volume, interrupted_note, retained_note},
};
use crate::{
    backend::Backend,
    cancel::Signal,
    cleanup::CleanupFailure,
    config_store::ConfigWriter,
    ownership::{Disposition, LedgerError, ResourceKind, ResourceLedger, ResourceRef},
    sync::CommandRunner,
    volume::{VolumeBackend, VolumeHandle},
};

impl<B, R, W> InitOrchestrator<B, R, W>
where
    B: Backend + VolumeBackend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
    W: ConfigWriter,
{
    /// Marks the created volume as retained, refusing to continue unless the
    /// ledger and journal agree.
    pub(super) fn promote_retained(
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
            Err(LedgerError::Persist { ref message }) => {
                // The journal still records the volume as owned, so letting
                // recovery replay it would delete the formatted cache. Report a
                // typed failure; `finish_success` runs helper cleanup with the
                // in-memory disposition still `Retained` and removes the
                // journal so recovery cannot delete the volume.
                return Err(after_volume(
                    InitError::Ledger(format!(
                        "the ledger could not record the retention: {message}"
                    )),
                    volume,
                    retained_note(&volume.id),
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
                InitError::Cleanup(CleanupFailure::new()),
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

    /// Cleans up helper resources when the volume's retention could not be
    /// recorded durably.
    ///
    /// The in-memory disposition is `Retained`, so cleanup preserves and
    /// detaches the volume while removing the formatter instance and its
    /// resources. The journal is then removed: its durable record still marks
    /// the volume as owned, and leaving it would let `mriya recover` delete the
    /// formatted cache.
    pub(super) async fn finish_unrecorded_retention(
        &self,
        context: &InitContext<'_>,
        volume: &VolumeHandle,
        error: InitError<B::Error>,
    ) -> InitError<B::Error> {
        let mut note = format!(
            "{retained}; the journal was removed so `mriya recover` cannot delete it",
            retained = retained_note(&volume.id)
        );
        match self.cleanup_retained(context, volume).await {
            Ok(warnings) => {
                if let Some(journal) = context.ledger.journal_path()
                    && context.ledger.clear_journal().is_err()
                {
                    note = format!(
                        "{retained}; the journal {journal} still records the volume as owned, \
                         so remove it before running `mriya recover`",
                        retained = retained_note(&volume.id)
                    );
                }
                for warning in warnings {
                    note.push_str("; note: ");
                    note.push_str(&warning);
                }
                after_volume(error, volume, note, None)
            }
            Err(cleanup) => after_volume(
                error,
                volume,
                format!(
                    "{retained}; cleanup also failed, so the journal remains and may still \
                     record the volume as owned: {cleanup}",
                    retained = retained_note(&volume.id)
                ),
                None,
            ),
        }
    }

    /// Runs the ownership-aware cleanup and keeps the warnings it reports.
    pub(super) async fn cleanup_retained(
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
                    InitError::Cleanup(failure.clone()),
                    volume,
                    note,
                    Some(failure),
                ));
            }
            CleanupOutcome::Interrupted => {
                return Err(after_volume(
                    InitError::Cleanup(CleanupFailure::new()),
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
    pub(super) fn cancelled_after_cleanup(
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
    pub(super) fn write_retained(
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
                InitError::Config(error),
                volume,
                retained_note(&volume.id),
                None,
            )),
        }
    }
}
