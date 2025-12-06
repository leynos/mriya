//! Orchestrates end-to-end remote runs over SSH.
//!
//! The run workflow provisions an instance via a backend, waits for SSH
//! readiness, synchronises the local workspace, executes a remote command
//! using the system `ssh` client, and tears the instance down. Remote exit
//! codes are preserved so callers observe the same status locally.

use std::fmt::Display;

use camino::Utf8Path;
use thiserror::Error;

use crate::backend::{Backend, InstanceHandle, InstanceRequest};
use crate::sync::{RemoteCommandOutput, SyncError, Syncer};

/// Errors surfaced while performing a remote run.
#[derive(Debug, Error)]
pub enum RunError<BackendError, SyncErr>
where
    BackendError: std::error::Error + 'static,
    SyncErr: std::error::Error + 'static,
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
    /// Raised when workspace synchronisation fails.
    #[error("workspace sync failed: {message}")]
    Sync {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronisation error.
        #[source]
        source: SyncErr,
    },
    /// Raised when the remote command fails to start.
    #[error("remote command failed to start: {message}")]
    Remote {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronisation error.
        #[source]
        source: SyncErr,
    },
    /// Raised when teardown fails after the primary operation succeeded.
    #[error("failed to destroy instance: {0}")]
    Teardown(#[source] BackendError),
}

/// Executes the remote run flow using the provided backend and syncer.
#[derive(Debug)]
pub struct RunOrchestrator<B, R: crate::sync::CommandRunner> {
    backend: B,
    syncer: Syncer<R>,
}

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: crate::sync::CommandRunner,
{
    /// Creates a new orchestrator.
    #[must_use]
    pub const fn new(backend: B, syncer: Syncer<R>) -> Self {
        Self { backend, syncer }
    }

    /// Runs the end-to-end workflow and returns the remote command output.
    ///
    /// The remote exit code is returned even when non-zero. Teardown is
    /// always attempted; when teardown fails the error is surfaced even if
    /// the remote command succeeded.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] when provisioning, readiness checks,
    /// synchronisation, remote execution, or teardown fail.
    pub async fn execute(
        &self,
        request: &InstanceRequest,
        source: &Utf8Path,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, RunError<B::Error, SyncError>> {
        let handle = self
            .backend
            .create(request)
            .await
            .map_err(RunError::Provision)?;

        let networking = match self.backend.wait_for_ready(&handle).await {
            Ok(net) => net,
            Err(err) => {
                return Err(self
                    .teardown_and_fail(handle, err, |message, wait_err| RunError::Wait {
                        message,
                        source: wait_err,
                    })
                    .await);
            }
        };

        let output = match self
            .syncer
            .sync(source, &self.syncer.destination_for(&networking))
        {
            Ok(()) => match self.syncer.run_remote(&networking, remote_command) {
                Ok(result) => result,
                Err(err) => {
                    return Err(self
                        .teardown_and_fail(handle, err, |message, remote_err| RunError::Remote {
                            message,
                            source: remote_err,
                        })
                        .await);
                }
            },
            Err(err) => {
                return Err(self
                    .teardown_and_fail(handle, err, |message, sync_err| RunError::Sync {
                        message,
                        source: sync_err,
                    })
                    .await);
            }
        };

        self.backend
            .destroy(handle)
            .await
            .map_err(RunError::Teardown)?;

        Ok(output)
    }

    async fn teardown_and_fail<E, F>(
        &self,
        handle: InstanceHandle,
        err: E,
        make_error: F,
    ) -> RunError<B::Error, SyncError>
    where
        E: Display,
        F: FnOnce(String, E) -> RunError<B::Error, SyncError>,
    {
        let teardown_error = self.backend.destroy(handle).await.err();
        let message = append_teardown_note(err.to_string(), teardown_error.as_ref());
        make_error(message, err)
    }
}

fn append_teardown_note<E: Display>(message: String, teardown_error: Option<&E>) -> String {
    if let Some(teardown) = teardown_error {
        format!("{message} (teardown also failed: {teardown})")
    } else {
        message
    }
}
