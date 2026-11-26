//! Cleanup and outcome composition for the run orchestrator.
//!
//! Every exit passes through one place: the successful path, the failure path,
//! and the cancellation path all call the same interruptible cleanup, then
//! compose the structured result the CLI prints.

use std::fmt::Display;

use camino::Utf8PathBuf;

use super::{
    CleanupOutcome,
    RunOrchestrator,
    error::{RunCancellation, RunCleanupFailure, RunError, RunOutcome},
};
use crate::{
    backend::Backend,
    cancel::{Cancellation, Signal},
    cleanup::CleanupReport,
    ownership::ResourceLedger,
    sync::{CommandRunner, RemoteCommandOutput},
};

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
{
    /// Returns cached provisioning output on success, or the cleanup failure.
    pub(super) async fn finish_success(
        &self,
        ledger: &ResourceLedger,
        completion: RunCompletion,
        cancellation: &Cancellation,
    ) -> Result<RunOutcome, RunError<B::Error>> {
        let RunCompletion {
            output,
            mut warnings,
        } = completion;
        match self.cleanup(ledger, cancellation).await {
            CleanupOutcome::Completed(report) => {
                collect_report_notes(&report, &mut warnings);
                let kept = clear_journal(ledger, &mut warnings);
                // A signal that arrived while cleanup ran still makes this an
                // interrupted run, even though every postcondition verified.
                if let Some(signal) = cancellation.signal() {
                    return Err(RunError::Cancelled(Box::new(RunCancellation {
                        signal,
                        cleanup: None,
                        journal: kept,
                        warnings,
                    })));
                }
                Ok(RunOutcome { output, warnings })
            }
            CleanupOutcome::Failed(error) => Err(RunError::Cleanup(Box::new(RunCleanupFailure {
                primary: None,
                output: Some(output),
                cleanup: Some(error),
                journal: ledger.journal_path(),
                interrupted: false,
                signal: cancellation.signal(),
                warnings,
            }))),
            CleanupOutcome::Interrupted => Err(RunError::Cleanup(Box::new(RunCleanupFailure {
                primary: None,
                output: Some(output),
                cleanup: None,
                journal: ledger.journal_path(),
                interrupted: true,
                signal: cancellation.signal(),
                warnings,
            }))),
        }
    }

    /// Cleans up after a failed step and returns the error to report.
    pub(super) async fn finish_failure(
        &self,
        ledger: &ResourceLedger,
        primary: RunError<B::Error>,
        cancellation: &Cancellation,
    ) -> RunError<B::Error> {
        let mut warnings = Vec::new();
        match self.cleanup(ledger, cancellation).await {
            CleanupOutcome::Completed(report) => {
                // Cleanup verified every postcondition, so the journal has
                // nothing left to recover; leaving it would make every later
                // `mriya recover` replay a settled allocation.
                warnings.extend(report.warnings);
                let kept = clear_journal(ledger, &mut warnings);
                if cancellation.signal().is_some() || kept.is_some() {
                    // The primary failure remains the reported error, but the
                    // notes gathered here must not be lost with it.
                    return RunError::Cleanup(Box::new(RunCleanupFailure {
                        primary: Some(Box::new(primary)),
                        output: None,
                        cleanup: None,
                        journal: kept,
                        interrupted: false,
                        signal: cancellation.signal(),
                        warnings,
                    }));
                }
                primary
            }
            CleanupOutcome::Failed(error) => RunError::Cleanup(Box::new(RunCleanupFailure {
                primary: Some(Box::new(primary)),
                output: None,
                cleanup: Some(error),
                journal: ledger.journal_path(),
                interrupted: false,
                signal: cancellation.signal(),
                warnings,
            })),
            CleanupOutcome::Interrupted => RunError::Cleanup(Box::new(RunCleanupFailure {
                primary: Some(Box::new(primary)),
                output: None,
                cleanup: None,
                journal: ledger.journal_path(),
                interrupted: true,
                signal: cancellation.signal(),
                warnings,
            })),
        }
    }

    /// Cleans up after a cancellation and returns the cancellation to report.
    pub(super) async fn finish_cancelled(
        &self,
        ledger: &ResourceLedger,
        cancellation: &Cancellation,
    ) -> RunError<B::Error> {
        let signal = cancellation.signal().unwrap_or(Signal::Interrupt);
        let mut warnings = Vec::new();
        match self.cleanup(ledger, cancellation).await {
            CleanupOutcome::Completed(report) => {
                warnings.extend(report.warnings);
                let kept = clear_journal(ledger, &mut warnings);
                RunError::Cancelled(Box::new(RunCancellation {
                    signal,
                    cleanup: None,
                    journal: kept,
                    warnings,
                }))
            }
            CleanupOutcome::Failed(error) => RunError::Cancelled(Box::new(RunCancellation {
                signal,
                cleanup: Some(Box::new(RunCleanupFailure {
                    primary: None,
                    output: None,
                    cleanup: Some(error),
                    journal: ledger.journal_path(),
                    interrupted: false,
                    signal: Some(signal),
                    warnings,
                })),
                journal: ledger.journal_path(),
                warnings: Vec::new(),
            })),
            CleanupOutcome::Interrupted => RunError::Cancelled(Box::new(RunCancellation {
                signal,
                cleanup: Some(Box::new(RunCleanupFailure {
                    primary: None,
                    output: None,
                    cleanup: None,
                    journal: ledger.journal_path(),
                    interrupted: true,
                    signal: Some(signal),
                    warnings,
                })),
                journal: ledger.journal_path(),
                warnings: Vec::new(),
            })),
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

/// Removes the journal after verified cleanup, noting a failure.
///
/// Returns the journal path when it remains on disk: either the outcome is
/// unknown (so recovery must reconcile the tag first) or the removal failed.
fn clear_journal(ledger: &ResourceLedger, warnings: &mut Vec<String>) -> Option<Utf8PathBuf> {
    if ledger.has_unknown_outcome() {
        let note = ledger
            .journal_path()
            .map_or_else(String::new, |path| format!(" at {path}"));
        warnings.push(format!(
            "an allocation outcome is unknown; kept the recovery journal{note}; \
             run `mriya recover` to reconcile it"
        ));
        return ledger.journal_path();
    }
    if let Err(error) = ledger.clear_journal() {
        warnings.push(format!(
            "could not remove the recovery journal: {message}",
            message = error.message
        ));
        return ledger.journal_path();
    }
    None
}

/// A completed remote command and the notes gathered before cleanup.
#[derive(Debug)]
pub struct RunCompletion {
    /// Output of the remote command.
    pub output: RemoteCommandOutput,
    /// Notes gathered before cleanup ran, such as a failed cache release.
    pub warnings: Vec<String>,
}

/// Adds cleanup warnings and preserved resource identifiers to `warnings`.
pub(super) fn collect_report_notes(report: &CleanupReport, warnings: &mut Vec<String>) {
    warnings.extend(report.warnings.iter().cloned());
    for resource in &report.preserved {
        warnings.push(format!("preserved {resource} and left it detached"));
    }
}
