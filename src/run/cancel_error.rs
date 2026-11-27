//! Signal-driven cancellation results for the run workflow.

use std::fmt;

use camino::Utf8PathBuf;

use super::error::RunCleanupFailure;
use crate::cancel::Signal;

/// Result of a signal-driven cancellation.
#[derive(Debug)]
pub struct RunCancellation<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// Signal that requested cancellation.
    pub signal: Signal,
    /// Cleanup outcome, absent when cleanup completed.
    pub cleanup: Option<Box<RunCleanupFailure<BackendError>>>,
    /// Journal that `mriya recover` can replay, when one remains.
    pub journal: Option<Utf8PathBuf>,
    /// Non-fatal notes gathered before and during cleanup.
    pub warnings: Vec<String>,
}

impl<BackendError> fmt::Display for RunCancellation<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.cleanup.is_some() {
            "ownership-aware cleanup did not complete"
        } else {
            "provisioned resources were cleaned up"
        };
        write!(formatter, "{} received; {state}", self.signal.as_str())?;
        if let Some(cleanup) = self.cleanup.as_deref() {
            write!(formatter, ": {cleanup}")?;
        }
        if let Some(journal) = self.journal.as_ref() {
            write!(
                formatter,
                "; run `mriya recover` to finish cleanup recorded in {journal}"
            )?;
        }
        for warning in &self.warnings {
            write!(formatter, "; note: {warning}")?;
        }
        Ok(())
    }
}

impl<BackendError> std::error::Error for RunCancellation<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cleanup
            .as_deref()
            .map(|cleanup| cleanup as &(dyn std::error::Error + 'static))
    }
}
