//! Errors and structured cleanup reporting for the run workflow.
//!
//! A cleanup failure must not hide the failure that triggered it, nor the
//! remote command's own status. These types keep all three parts — primary
//! error, completed remote output, and typed cleanup failure with residual
//! resource identifiers — so the CLI can print them together.

use std::fmt;

use camino::Utf8PathBuf;
use thiserror::Error;

pub use super::cancel_error::RunCancellation;
use crate::{
    cancel::Signal,
    cleanup::CleanupFailure,
    sync::{RemoteCommandOutput, SyncError},
};

/// Errors surfaced while performing a remote run.
#[derive(Debug, Error)]
pub enum RunError<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// Raised when provisioning a new instance fails.
    #[error("failed to create instance: {0}")]
    Provision(#[source] BackendError),
    /// Raised when the instance does not become reachable over SSH.
    #[error("instance did not become ready: {message}")]
    Wait {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: BackendError,
    },
    /// Raised when cloud-init provisioning does not complete.
    #[error("instance provisioning did not complete: {message}")]
    Provisioning {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronization error.
        #[source]
        source: SyncError,
    },
    /// Raised when cloud-init provisioning does not complete before the timeout.
    #[error("instance provisioning did not complete: {message}")]
    ProvisioningTimeout {
        /// Human-readable description of the failure.
        message: String,
    },
    /// Raised when workspace synchronization fails.
    #[error("workspace sync failed: {message}")]
    Sync {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronization error.
        #[source]
        source: SyncError,
    },
    /// Raised when the remote command fails to start.
    #[error("remote command failed to start: {message}")]
    Remote {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronization error.
        #[source]
        source: SyncError,
    },
    /// Raised when ownership-aware cleanup cannot verify its postconditions.
    ///
    /// The wrapper is its own source, so a chain walker reaches the typed
    /// [`CleanupFailure`] rather than stopping at the outer error. `Display`
    /// is the wrapper's message, which already renders the primary error.
    #[error("{failure}")]
    Cleanup {
        /// Everything known when cleanup could not verify its postconditions.
        #[source]
        failure: Box<RunCleanupFailure<BackendError>>,
    },
    /// Raised when a signal interrupted the run.
    ///
    /// The variant is distinct so the CLI can exit 130 or 143 rather than 1.
    #[error("{0}")]
    Cancelled(#[source] Box<RunCancellation<BackendError>>),
}

/// Everything known when cleanup could not verify its postconditions.
#[derive(Debug)]
pub struct RunCleanupFailure<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// The failure that triggered cleanup, when there was one.
    pub primary: Option<Box<RunError<BackendError>>>,
    /// Output of a remote command that completed before cleanup ran.
    pub output: Option<RemoteCommandOutput>,
    /// Typed cleanup failure, absent when a second signal interrupted cleanup.
    pub cleanup: Option<CleanupFailure>,
    /// Whether cleanup verified every postcondition.
    ///
    /// A failure that merely carries notes — a signal during cleanup, or a
    /// journal that could not be removed — has verified cleanup, and the
    /// message must not call it incomplete.
    pub cleanup_verified: bool,
    /// Journal that `mriya recover` can replay.
    pub journal: Option<Utf8PathBuf>,
    /// Whether a second signal stopped cleanup early.
    pub interrupted: bool,
    /// A signal that arrived while cleanup was running.
    pub signal: Option<Signal>,
    /// Non-fatal notes gathered before and during cleanup.
    pub warnings: Vec<String>,
}

impl<BackendError> RunCleanupFailure<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// Describes the cleanup part of the failure.
    fn cleanup_message(&self) -> String {
        if self.interrupted {
            return String::from(
                "cleanup was interrupted by a second signal; run `mriya recover` to finish it",
            );
        }
        if let Some(error) = self.cleanup.as_ref() {
            return format!("cleanup failed: {error}");
        }
        if self.cleanup_verified {
            return String::from("cleanup verified every postcondition");
        }
        String::from("cleanup did not complete")
    }

    /// Describes the remote command's status, when one ran.
    fn output_message(&self) -> Option<String> {
        self.output.as_ref().map(|output| {
            output.exit_code.map_or_else(
                || String::from("remote command terminated without an exit status"),
                |code| format!("remote command exited with status {code}"),
            )
        })
    }
}

impl<BackendError> fmt::Display for RunCleanupFailure<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if let (true, Some(primary)) = (self.cleanup_verified, self.primary.as_deref()) {
            parts.push(primary.to_string());
            parts.push(self.cleanup_message());
        } else {
            parts.push(self.cleanup_message());
            if let Some(primary) = self.primary.as_deref() {
                parts.push(format!("after: {primary}"));
            }
        }
        if let Some(output) = self.output_message() {
            parts.push(output);
        }
        if let Some(journal) = self.journal.as_ref() {
            parts.push(format!("journal: {journal}"));
        }
        if let Some(signal) = self.signal {
            parts.push(format!("{} arrived during cleanup", signal.as_str()));
        }
        formatter.write_str(&parts.join("; "))?;
        for warning in &self.warnings {
            write!(formatter, "; note: {warning}")?;
        }
        Ok(())
    }
}

impl<BackendError> std::error::Error for RunCleanupFailure<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cleanup
            .as_ref()
            .map(|error| error as &(dyn std::error::Error + 'static))
    }
}

/// Successful run result, including notes the CLI prints on standard error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunOutcome {
    /// Output of the remote command.
    pub output: RemoteCommandOutput,
    /// Non-fatal notes, such as a cache volume that could not be unmounted.
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    //! Unit tests for composing the structured failure messages.

    use rstest::rstest;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("backend said no")]
    struct Backend;

    fn output(code: Option<i32>) -> RemoteCommandOutput {
        RemoteCommandOutput {
            exit_code: code,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[rstest]
    fn cleanup_failure_renders_every_part() {
        let mut cleanup = CleanupFailure::new();
        cleanup.add_cause("backend said no");
        let failure: RunCleanupFailure<Backend> = RunCleanupFailure {
            primary: Some(Box::new(RunError::Provision(Backend))),
            output: Some(output(Some(7))),
            cleanup: Some(cleanup),
            cleanup_verified: false,
            journal: Some(Utf8PathBuf::from("/var/state/mriya/runs/a.json")),
            interrupted: false,
            signal: None,
            warnings: Vec::new(),
        };
        let rendered = failure.to_string();
        assert!(
            rendered.contains("cleanup failed: backend said no"),
            "{rendered}"
        );
        assert!(
            rendered.contains("after: failed to create instance"),
            "{rendered}"
        );
        assert!(
            rendered.contains("remote command exited with status 7"),
            "{rendered}"
        );
        assert!(
            rendered.contains("/var/state/mriya/runs/a.json"),
            "{rendered}"
        );
    }

    #[rstest]
    fn an_interrupted_cleanup_names_recover() {
        let failure: RunCleanupFailure<Backend> = RunCleanupFailure {
            primary: None,
            output: None,
            cleanup: None,
            cleanup_verified: false,
            journal: Some(Utf8PathBuf::from("/tmp/journal.json")),
            interrupted: true,
            signal: None,
            warnings: Vec::new(),
        };
        assert!(failure.to_string().contains("mriya recover"));
    }

    #[rstest]
    fn a_cleanup_error_exposes_the_typed_failure_as_its_source() {
        use std::error::Error as _;

        let mut typed_cleanup = CleanupFailure::new();
        typed_cleanup.add_cause("backend said no");
        let error: RunError<Backend> = RunError::Cleanup {
            failure: Box::new(RunCleanupFailure {
                primary: None,
                output: None,
                cleanup: Some(typed_cleanup),
                cleanup_verified: false,
                journal: None,
                interrupted: false,
                signal: None,
                warnings: Vec::new(),
            }),
        };
        let mut source = error.source();
        let mut reached_cleanup = false;
        while let Some(current) = source {
            if current.downcast_ref::<CleanupFailure>().is_some() {
                reached_cleanup = true;
                break;
            }
            source = current.source();
        }
        assert!(
            reached_cleanup,
            "the source chain must reach the typed CleanupFailure"
        );
    }

    #[rstest]
    fn a_cancellation_exposes_its_cleanup_failure_as_a_source() {
        use std::error::Error as _;

        let mut typed_cleanup = CleanupFailure::new();
        typed_cleanup.add_cause("backend said no");
        let error: RunError<Backend> = RunError::Cancelled(Box::new(RunCancellation {
            signal: Signal::Interrupt,
            cleanup: Some(Box::new(RunCleanupFailure {
                primary: None,
                output: None,
                cleanup: Some(typed_cleanup),
                cleanup_verified: false,
                journal: None,
                interrupted: false,
                signal: Some(Signal::Interrupt),
                warnings: Vec::new(),
            })),
            journal: None,
            warnings: Vec::new(),
        }));
        let mut source = error.source();
        let mut reached_cleanup = false;
        while let Some(current) = source {
            if current.downcast_ref::<CleanupFailure>().is_some() {
                reached_cleanup = true;
                break;
            }
            source = current.source();
        }
        assert!(
            reached_cleanup,
            "the cancellation source chain must reach the typed CleanupFailure"
        );
    }

    #[rstest]
    fn a_cancellation_without_cleanup_failure_reports_success() {
        let cancellation: RunCancellation<Backend> = RunCancellation {
            signal: Signal::Interrupt,
            cleanup: None,
            journal: None,
            warnings: Vec::new(),
        };
        assert_eq!(
            cancellation.to_string(),
            "SIGINT received; provisioned resources were cleaned up"
        );
        assert_eq!(cancellation.signal.exit_code(), 130);
    }
}
