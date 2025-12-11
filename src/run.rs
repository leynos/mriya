//! Orchestrates end-to-end remote runs over SSH.
//!
//! The run workflow provisions an instance via a backend, waits for SSH
//! readiness, synchronises the local workspace, executes a remote command
//! using the system `ssh` client, and tears the instance down. Remote exit
//! codes are preserved so callers observe the same status locally.

use std::fmt::Display;

use camino::Utf8Path;
use thiserror::Error;

use crate::backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest};
use crate::sync::{CommandRunner, RemoteCommandOutput, SyncError, Syncer};

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
    /// Raised when workspace synchronisation fails.
    #[error("workspace sync failed: {message}")]
    Sync {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronisation error.
        #[source]
        source: SyncError,
    },
    /// Raised when the remote command fails to start.
    #[error("remote command failed to start: {message}")]
    Remote {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronisation error.
        #[source]
        source: SyncError,
    },
    /// Raised when teardown fails after the primary operation succeeded.
    #[error("failed to destroy instance: {0}")]
    Teardown(#[source] BackendError),
}

/// Executes the remote run flow using the provided backend and syncer.
#[derive(Debug)]
pub struct RunOrchestrator<B, R: CommandRunner> {
    backend: B,
    syncer: Syncer<R>,
}

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
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
    ) -> Result<RemoteCommandOutput, RunError<B::Error>> {
        let handle = self
            .backend
            .create(request)
            .await
            .map_err(RunError::Provision)?;

        let networking = match self.backend.wait_for_ready(&handle).await {
            Ok(net) => net,
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                return Err(RunError::Wait {
                    message,
                    source: err,
                });
            }
        };

        // Mount cache volume if configured
        if request.volume_id.is_some() {
            self.mount_cache_volume(&handle, &networking).await?;
        }

        let dest = self.syncer.destination_for(&networking);

        if let Err(err) = self.syncer.sync(source, &dest) {
            let message = self.destroy_with_note(handle, &err).await;
            return Err(RunError::Sync {
                message,
                source: err,
            });
        }

        let output = match self.syncer.run_remote(&networking, remote_command) {
            Ok(result) => result,
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                return Err(RunError::Remote {
                    message,
                    source: err,
                });
            }
        };

        self.backend
            .destroy(handle)
            .await
            .map_err(RunError::Teardown)?;

        Ok(output)
    }

    /// Mounts the cache volume via SSH.
    ///
    /// The mount command is idempotent: it creates the mount point directory
    /// and attempts to mount `/dev/vdb`. Failures are logged but not surfaced
    /// as errors to allow runs to proceed when the volume is already mounted
    /// or formatted differently.
    async fn mount_cache_volume(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
    ) -> Result<(), RunError<B::Error>> {
        let mount_path = &self.syncer.config().volume_mount_path;
        let mount_command = format!(
            concat!(
                "sudo mkdir -p {path} && ",
                "sudo mount /dev/vdb {path} 2>/dev/null || true"
            ),
            path = mount_path
        );

        match self.syncer.run_remote(networking, &mount_command) {
            Ok(_) => Ok(()),
            Err(err) => {
                let message = self.destroy_with_note(handle.clone(), &err).await;
                Err(RunError::Sync {
                    message,
                    source: err,
                })
            }
        }
    }

    async fn destroy_with_note<E: Display>(&self, handle: InstanceHandle, err: &E) -> String {
        let teardown_error = self.backend.destroy(handle).await.err();
        append_teardown_note(err.to_string(), teardown_error.as_ref())
    }
}

fn append_teardown_note<E: Display>(message: String, teardown_error: Option<&E>) -> String {
    if let Some(teardown) = teardown_error {
        format!("{message} (teardown also failed: {teardown})")
    } else {
        message
    }
}
