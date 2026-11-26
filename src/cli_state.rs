//! Ledger creation and the `mriya recover` command.
//!
//! `run` and `init` open a journal-backed ledger before provisioning, and
//! `recover` replays every journal left behind. Recovery never deletes a
//! retained volume: it verifies the volume is present and detached, prints the
//! configuration setting that would make it reusable, and removes the journal
//! only after cleanup verified its postconditions.

use std::{
    io::{self, Write},
    sync::Arc,
};

use mriya::{
    AllocationScope,
    AllocationTag,
    Backend,
    Cancellation,
    ConfigWriter,
    JournalStore,
    LedgerSink,
    ResourceLedger,
    ScalewayBackend,
    ScalewayConfig,
    Signal,
    until_cancelled,
};

use crate::CliError;

/// Opens a journal-backed ledger for a new allocation.
///
/// # Errors
///
/// Returns [`CliError::Config`] when the state directory cannot be resolved.
pub(crate) fn open_ledger(scope: AllocationScope) -> Result<ResourceLedger, CliError> {
    let store = JournalStore::from_env(&mockable::DefaultEnv)
        .map_err(|error| CliError::Config(error.to_string()))?;
    let tag = AllocationTag::generate();
    let journal = store.handle(&tag);
    let sink: Arc<dyn LedgerSink> = Arc::new(journal);
    Ok(ResourceLedger::with_tag(scope, tag, Some(sink)))
}

/// Warns on standard error when earlier runs left recovery journals behind.
pub(crate) fn warn_about_pending_journals() {
    let Ok(store) = JournalStore::from_env(&mockable::DefaultEnv) else {
        return;
    };
    let Ok(paths) = store.journals() else {
        return;
    };
    if paths.is_empty() {
        return;
    }
    let mut stderr = io::stderr();
    writeln!(
        stderr,
        "warning: {} recovery journal(s) from earlier runs remain in {}; run `mriya recover` to clean them up",
        paths.len(),
        store.root()
    )
    .ok();
}

/// Replays every recovery journal and reports the outcome.
///
/// A journal whose owning `mriya run` is still alive is skipped: that run is
/// rewriting it and using the resources it names. The retained cache
/// identifiers a recovery preserves must already be recorded in configuration
/// before the journal is removed, so a journal that only carries the ID stays
/// available to `mriya recover` after the configuration is fixed.
///
/// # Errors
///
/// Returns [`CliError`] when configuration, the journal directory, or the
/// backend cannot be used. Residual resources are reported as a failed result
/// rather than as a configuration error.
pub(crate) async fn recover_command(cancellation: &Cancellation) -> Result<i32, CliError> {
    let store = JournalStore::from_env(&mockable::DefaultEnv)
        .map_err(|error| CliError::Config(error.to_string()))?;
    let paths = store
        .journals()
        .map_err(|error| CliError::Config(error.to_string()))?;

    let mut stdout = io::stdout();
    if paths.is_empty() {
        writeln!(stdout, "no recovery journals found in {}", store.root()).ok();
        return Ok(0);
    }

    let config = ScalewayConfig::load_without_cli_args()
        .map_err(|error| CliError::Config(error.to_string()))?;
    let backend = ScalewayBackend::new(config.clone())
        .map_err(|error| CliError::Backend(error.to_string()))?;
    let configured = configured_volume_id(&config);
    let recovery = Recovery {
        store: &store,
        backend: &backend,
        configured: configured.as_deref(),
    };

    let mut failed = false;
    for path in paths {
        if let Some(signal) = cancellation.signal() {
            writeln!(
                stdout,
                "{} received; stopped before replaying {path}",
                signal.as_str()
            )
            .ok();
            return Ok(signal.exit_code());
        }
        match recovery.replay_path(&path, cancellation, &mut stdout).await {
            ReplayStep::Settled | ReplayStep::Skipped => {}
            ReplayStep::Kept(reason) => {
                failed = true;
                writeln!(stdout, "journal kept at {path}: {reason}").ok();
            }
            ReplayStep::Interrupted(signal) => {
                writeln!(
                    stdout,
                    "{} received while replaying {path}; the journal is kept",
                    signal.as_str()
                )
                .ok();
                return Ok(signal.exit_code());
            }
        }
    }

    if failed { Ok(1) } else { Ok(0) }
}

/// Outcome of one journal's claim, owner check, and replay.
enum ReplayStep {
    /// The journal was replayed and removed.
    Settled,
    /// The journal was skipped because another process owns it.
    Skipped,
    /// The journal remains, with the reason it must stay.
    Kept(String),
    /// A signal stopped the replay.
    Interrupted(Signal),
}

impl Recovery<'_> {
    /// Claims, checks, and replays one journal.
    async fn replay_path(
        &self,
        path: &camino::Utf8Path,
        cancellation: &Cancellation,
        stdout: &mut impl Write,
    ) -> ReplayStep {
        // The claim is acquired before the owner check: it excludes a second
        // recover process for the whole check-and-replay window, which a plain
        // re-read cannot do.
        let claim = match self.store.claim(path) {
            Ok(Some(claim)) => claim,
            Ok(None) => {
                writeln!(stdout, "skipping {path}: another recovery holds its claim").ok();
                return ReplayStep::Skipped;
            }
            Err(error) => {
                return ReplayStep::Kept(format!("the claim could not be taken: {error}"));
            }
        };
        let step = match self.store.load(path) {
            Ok(record) if record.owner.is_some_and(|owner| owner.is_alive()) => {
                writeln!(stdout, "skipping {path}: its owning run is still alive").ok();
                ReplayStep::Skipped
            }
            Ok(_) => match self.replay(path, cancellation, stdout).await {
                ReplayOutcome::Settled => ReplayStep::Settled,
                ReplayOutcome::Kept(reason) => ReplayStep::Kept(reason),
                ReplayOutcome::Interrupted(signal) => ReplayStep::Interrupted(signal),
            },
            Err(error) => ReplayStep::Kept(format!("could not read {path}: {error}")),
        };
        drop(claim);
        step
    }
}

/// The state shared by every journal replay in one command.
struct Recovery<'a> {
    /// Journals to open and remove.
    store: &'a JournalStore,
    /// Backend that performs the ownership-aware cleanup.
    backend: &'a ScalewayBackend,
    /// Cache volume identifier configured outside the journal, if any.
    configured: Option<&'a str>,
}

/// Result of replaying one journal.
enum ReplayOutcome {
    /// Cleanup verified every postcondition and the journal is removed.
    Settled,
    /// The journal remains, with the reason it must stay.
    Kept(String),
    /// A signal stopped the replay; the journal remains and the process exits
    /// with the signal's status.
    Interrupted(Signal),
}

impl Recovery<'_> {
    /// Replays one journal, keeping it when cleanup cannot be verified.
    ///
    /// A second signal stops the replay: cleanup is dropped mid-flight and the
    /// journal is kept, so the run that is being interrupted can be finished
    /// later by `mriya recover` rather than half-cleaned now.
    async fn replay(
        &self,
        path: &camino::Utf8Path,
        cancellation: &Cancellation,
        stdout: &mut impl Write,
    ) -> ReplayOutcome {
        let Ok((record, sink)) = self.store.open(path) else {
            return ReplayOutcome::Kept(String::from("the journal could not be opened"));
        };
        let tag = record.allocation.clone();
        let ledger = record.into_ledger(sink);
        let Some(cleanup) = until_cancelled(self.backend.cleanup(&ledger), cancellation).await
        else {
            return ReplayOutcome::Interrupted(cancellation.signal().unwrap_or(Signal::Interrupt));
        };
        let report = match cleanup {
            Ok(report) => report,
            Err(failure) => {
                writeln!(stdout, "recovery of {tag} failed: {failure}").ok();
                return ReplayOutcome::Kept(String::from(
                    "cleanup could not verify every resource",
                ));
            }
        };
        report_recovered(stdout, &tag, &report, self.configured);
        let unrecorded = report
            .preserved
            .iter()
            .any(|resource| self.configured != Some(resource.id()));
        if unrecorded {
            return ReplayOutcome::Kept(String::from(
                "a retained volume is not recorded in configuration yet",
            ));
        }
        if let Err(error) = ledger.resolve_outcome() {
            return ReplayOutcome::Kept(format!(
                "the resolved outcome could not be written: {message}",
                message = error.message
            ));
        }
        if let Err(error) = ledger.clear_journal() {
            return ReplayOutcome::Kept(format!(
                "the journal could not be removed: {message}",
                message = error.message
            ));
        }
        ReplayOutcome::Settled
    }
}

/// Returns the cache volume ID configured outside the journal, if any.
fn configured_volume_id(config: &ScalewayConfig) -> Option<String> {
    let from_file = mriya::ConfigStore::new().current_volume_id().ok().flatten();
    from_file.or_else(|| config.default_volume_id.clone())
}

/// Prints the result of one verified recovery.
fn report_recovered(
    stdout: &mut impl Write,
    tag: &AllocationTag,
    report: &mriya::CleanupReport,
    configured: Option<&str>,
) {
    writeln!(stdout, "recovered {tag}").ok();
    for resource in &report.removed {
        writeln!(stdout, "  removed {resource}").ok();
    }
    for resource in &report.preserved {
        writeln!(
            stdout,
            "  retained {resource}; configure it with [scaleway] default_volume_id = \"{id}\" \
             or SCW_DEFAULT_VOLUME_ID={id}",
            id = resource.id()
        )
        .ok();
        if configured != Some(resource.id()) {
            writeln!(
                stdout,
                "  the journal is kept until {id} is recorded in configuration",
                id = resource.id()
            )
            .ok();
        }
    }
    for warning in &report.warnings {
        writeln!(stdout, "  warning: {warning}").ok();
    }
}
