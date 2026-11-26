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
    JournalStore,
    LedgerSink,
    ResourceLedger,
    ScalewayBackend,
    ScalewayConfig,
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
/// # Errors
///
/// Returns [`CliError`] when configuration, the journal directory, or the
/// backend cannot be used. Residual resources are reported as a failed result
/// rather than as a configuration error.
pub(crate) async fn recover_command() -> Result<i32, CliError> {
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
    let backend =
        ScalewayBackend::new(config).map_err(|error| CliError::Backend(error.to_string()))?;

    let mut failed = false;
    for path in paths {
        match store.open(&path) {
            Ok((record, sink)) => {
                let tag = record.allocation.clone();
                let ledger = record.into_ledger(sink);
                match backend.cleanup(&ledger).await {
                    Ok(report) => report_recovered(&mut stdout, &tag, &report, &ledger),
                    Err(failure) => {
                        failed = true;
                        writeln!(stdout, "recovery of {tag} failed: {failure}").ok();
                        writeln!(stdout, "journal kept at {path}").ok();
                    }
                }
            }
            Err(error) => {
                failed = true;
                writeln!(stdout, "could not read {path}: {error}").ok();
            }
        }
    }

    if failed { Ok(1) } else { Ok(0) }
}

/// Prints the result of one verified recovery.
fn report_recovered(
    stdout: &mut impl Write,
    tag: &AllocationTag,
    report: &mriya::CleanupReport,
    ledger: &ResourceLedger,
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
    }
    for warning in &report.warnings {
        writeln!(stdout, "  warning: {warning}").ok();
    }
    if let Err(error) = ledger.clear_journal() {
        writeln!(
            stdout,
            "  warning: could not remove the journal: {message}",
            message = error.message
        )
        .ok();
    }
}
