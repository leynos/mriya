//! Orchestrates end-to-end remote runs over SSH.
//!
//! The run workflow provisions an instance via a backend, waits for SSH
//! readiness, synchronises the local workspace, executes a remote command
//! using the system `ssh` client, and tears the instance down. Remote exit
//! codes are preserved so callers observe the same status locally.

use std::fmt::Display;
use std::time::{Duration, Instant};

use camino::Utf8Path;
use shell_escape::unix::escape;
use thiserror::Error;
use tokio::time::sleep;

use crate::backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest};
use crate::sync::{CommandRunner, RemoteCommandOutput, SyncError, Syncer};

const CLOUD_INIT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const CLOUD_INIT_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

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
        /// Underlying synchronisation error.
        #[source]
        source: SyncError,
    },
    /// Raised when cloud-init provisioning does not complete before the timeout.
    #[error("instance provisioning did not complete: {message}")]
    ProvisioningTimeout {
        /// Human-readable description of the failure.
        message: String,
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
    cloud_init_poll_interval: Duration,
    cloud_init_wait_timeout: Duration,
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
        Self {
            backend,
            syncer,
            cloud_init_poll_interval: CLOUD_INIT_POLL_INTERVAL,
            cloud_init_wait_timeout: CLOUD_INIT_WAIT_TIMEOUT,
        }
    }

    /// Overrides the cloud-init polling interval.
    ///
    /// This is primarily used by tests to keep timeout scenarios fast.
    #[must_use]
    pub const fn with_cloud_init_poll_interval(mut self, interval: Duration) -> Self {
        self.cloud_init_poll_interval = interval;
        self
    }

    /// Overrides the cloud-init wait timeout.
    ///
    /// This is primarily used by tests to keep timeout scenarios fast.
    #[must_use]
    pub const fn with_cloud_init_wait_timeout(mut self, timeout: Duration) -> Self {
        self.cloud_init_wait_timeout = timeout;
        self
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
        let networking = self.wait_for_ready_or_destroy(&handle).await?;

        self.mount_volume_if_needed(&handle, &networking, request)
            .await?;

        let dest = self.syncer.destination_for(&networking);
        self.sync_or_destroy(&handle, source, &dest).await?;

        if request.cloud_init_user_data.is_some() {
            self.wait_for_cloud_init(&handle, &networking).await?;
        }

        let output = self
            .run_remote_or_destroy(&handle, &networking, remote_command)
            .await?;

        self.backend
            .destroy(handle)
            .await
            .map_err(RunError::Teardown)?;

        Ok(output)
    }

    async fn wait_for_ready_or_destroy(
        &self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, RunError<B::Error>> {
        match self.backend.wait_for_ready(handle).await {
            Ok(net) => Ok(net),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(RunError::Wait {
                    message,
                    source: err,
                })
            }
        }
    }

    async fn mount_volume_if_needed(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        request: &InstanceRequest,
    ) -> Result<(), RunError<B::Error>> {
        if request.volume_id.is_some() {
            self.mount_cache_volume(handle, networking).await?;
        }
        Ok(())
    }

    async fn sync_or_destroy(
        &self,
        handle: &InstanceHandle,
        source: &Utf8Path,
        dest: &crate::sync::SyncDestination,
    ) -> Result<(), RunError<B::Error>> {
        if let Err(err) = self.syncer.sync(source, dest) {
            let message = self.destroy_with_note(handle, &err).await;
            return Err(RunError::Sync {
                message,
                source: err,
            });
        }
        Ok(())
    }

    async fn run_remote_or_destroy(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, RunError<B::Error>> {
        match self.syncer.run_remote(networking, remote_command) {
            Ok(result) => Ok(result),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(RunError::Remote {
                    message,
                    source: err,
                })
            }
        }
    }

    /// Mounts the cache volume via SSH.
    ///
    /// The mount command is idempotent: it creates the mount point directory
    /// and attempts to mount `/dev/vdb`. The mount itself is best-effort
    /// because the command uses `|| true` for graceful degradation. Only SSH
    /// execution failures are surfaced as errors.
    async fn mount_cache_volume(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
    ) -> Result<(), RunError<B::Error>> {
        let mount_path = &self.syncer.config().volume_mount_path;
        let escaped_mount_path = escape(mount_path.as_str().into());
        let mount_command = format!(
            concat!(
                "sudo mkdir -p {path} && ",
                "sudo mount /dev/vdb {path} 2>/dev/null || true"
            ),
            path = escaped_mount_path
        );

        match self.syncer.run_remote(networking, &mount_command) {
            Ok(_) => Ok(()),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(RunError::Sync {
                    message,
                    source: err,
                })
            }
        }
    }

    async fn wait_for_cloud_init(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
    ) -> Result<(), RunError<B::Error>> {
        let deadline = Instant::now() + self.cloud_init_wait_timeout;
        let cloud_init_finished_marker = "/var/lib/cloud/instance/boot-finished";
        let command = format!("sudo test -f {cloud_init_finished_marker}");

        while Instant::now() <= deadline {
            let finished = match self.syncer.run_remote(networking, &command) {
                Ok(output) => matches!(output.exit_code, Some(0)),
                Err(err) => {
                    let message = self.destroy_with_note(handle, &err).await;
                    return Err(RunError::Provisioning {
                        message,
                        source: err,
                    });
                }
            };

            if finished {
                return Ok(());
            }

            sleep(self.cloud_init_poll_interval).await;
        }

        let timeout_message = format!(
            "cloud-init did not finish within {} seconds",
            self.cloud_init_wait_timeout.as_secs()
        );
        let message_with_teardown = self.destroy_with_note(handle, &timeout_message).await;
        Err(RunError::ProvisioningTimeout {
            message: message_with_teardown,
        })
    }

    async fn destroy_with_note<E: Display>(&self, handle: &InstanceHandle, err: &E) -> String {
        let teardown_error = self.backend.destroy(handle.clone()).await.err();
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
