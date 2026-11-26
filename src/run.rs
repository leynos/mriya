//! Orchestrates end-to-end remote runs over SSH.
//!
//! The run workflow provisions an instance via a backend, waits for SSH
//! readiness, synchronizes the local workspace, executes a remote command
//! using the system `ssh` client, and tears the instance down. Remote exit
//! codes are preserved so callers observe the same status locally.

use std::{
    fmt::Display,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use shell_escape::unix::escape;
use thiserror::Error;
use tokio::time::sleep;

use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest},
    cache_device::{self, CacheDeviceStatus},
    sync::{
        CommandRunner,
        RemoteCommandOutput,
        SyncError,
        Syncer,
        create_cache_directories_command,
    },
    teardown::append_teardown_note,
    volume::VolumeBackend,
};

/// Delay between checks for the cloud-init completion marker.
const CLOUD_INIT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Total time to wait for cloud-init to finish (ten minutes).
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
    /// Raised when a configured cache cannot be mounted or verified.
    #[error("failed to mount the configured cache volume: {message}")]
    CacheMount {
        /// Human-readable description of the refusal.
        message: String,
        /// Underlying synchronization error, when the SSH call itself failed.
        #[source]
        source: Option<SyncError>,
    },
    /// Raised when teardown fails after the primary operation succeeded.
    #[error("failed to destroy instance: {message}")]
    Teardown {
        /// Human-readable description, including any unmount failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: BackendError,
    },
}

/// Executes the remote run flow using the provided backend and syncer.
#[derive(Debug)]
pub struct RunOrchestrator<B, R: CommandRunner> {
    /// Backend that creates, waits for, and destroys the instance.
    backend: B,
    /// Syncer used for workspace transfer and remote command execution.
    syncer: Syncer<R>,
    /// Delay between cloud-init completion checks.
    cloud_init_poll_interval: Duration,
    /// Time allowed for cloud-init before a `ProvisioningTimeout` error.
    cloud_init_wait_timeout: Duration,
}

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend + VolumeBackend,
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
    /// synchronization, remote execution, or teardown fail.
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
        let networking = self
            .wait_for_ready_or_destroy(&handle, request.volume_id.as_deref())
            .await?;

        let cache_verified = self
            .mount_volume_if_needed(&handle, &networking, request)
            .await?;

        let dest = self.syncer.destination_for(&networking);
        self.sync_or_destroy(
            &handle,
            &networking,
            source,
            &dest,
            request.volume_id.as_deref(),
        )
        .await?;

        if request.cloud_init_user_data.is_some() {
            self.wait_for_cloud_init(&handle, &networking, request.volume_id.as_deref())
                .await?;
        }

        let output = self
            .run_remote_or_destroy(
                &handle,
                &networking,
                remote_command,
                request.volume_id.as_deref(),
                cache_verified,
            )
            .await?;

        let unmount_note = self
            .unmount_cache(&networking, request.volume_id.as_deref())
            .await;
        if let Err(source) = self
            .backend
            .teardown(&handle, request.volume_id.as_deref())
            .await
        {
            let message = match unmount_note {
                Some(note) => format!("{source} ({note})"),
                None => source.to_string(),
            };
            return Err(RunError::Teardown { message, source });
        }

        Ok(output)
    }

    /// Waits for the backend to report SSH readiness, destroying the instance
    /// if that fails.
    ///
    /// The teardown outcome is appended to the error message.
    ///
    /// For example, an SSH timeout returns its own error with
    /// `(teardown also failed: ...)` appended when the destroy also fails, so
    /// the caller sees both problems rather than only the first.
    async fn wait_for_ready_or_destroy(
        &self,
        handle: &InstanceHandle,
        volume_id: Option<&str>,
    ) -> Result<InstanceNetworking, RunError<B::Error>> {
        match self.backend.wait_for_ready(handle).await {
            Ok(net) => Ok(net),
            Err(err) => {
                let message = self.teardown_with_note(handle, volume_id, None, &err).await;
                Err(RunError::Wait {
                    message,
                    source: err,
                })
            }
        }
    }

    /// Mounts the cache volume only when the request attached one.
    ///
    /// Returns whether cache routing is allowed. A request without an attached
    /// volume keeps the historical behaviour: the preamble is emitted with its
    /// `mountpoint -q` guard. A request with a volume is allowed to route only
    /// after the mount is verified.
    async fn mount_volume_if_needed(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        request: &InstanceRequest,
    ) -> Result<bool, RunError<B::Error>> {
        let Some(volume_id) = request.volume_id.as_deref() else {
            return Ok(true);
        };
        self.mount_cache_volume(handle, networking, volume_id)
            .await?;
        Ok(true)
    }

    /// Syncs the workspace to `dest`, destroying the instance if that fails.
    ///
    /// For example, a failed rsync returns the sync error with the teardown
    /// outcome appended, so a leaked instance is never reported as a plain
    /// sync failure.
    async fn sync_or_destroy(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        source: &Utf8Path,
        dest: &crate::sync::SyncDestination,
        volume_id: Option<&str>,
    ) -> Result<(), RunError<B::Error>> {
        if let Err(err) = self.syncer.sync(source, dest) {
            let message = self
                .teardown_with_note(handle, volume_id, Some(networking), &err)
                .await;
            return Err(RunError::Sync {
                message,
                source: err,
            });
        }
        Ok(())
    }

    /// Runs `remote_command`, destroying the instance only if it could not be
    /// started.
    ///
    /// A non-zero exit code from the command is a successful result here.
    ///
    /// For example, a command exiting 7 returns that status in
    /// [`RemoteCommandOutput`], while a failure to start the command destroys
    /// the instance and returns the error text.
    async fn run_remote_or_destroy(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        remote_command: &str,
        volume_id: Option<&str>,
        cache_verified: bool,
    ) -> Result<RemoteCommandOutput, RunError<B::Error>> {
        match self
            .syncer
            .run_remote_with_cache(networking, remote_command, cache_verified)
        {
            Ok(result) => Ok(result),
            Err(err) => {
                let message = self
                    .teardown_with_note(handle, volume_id, Some(networking), &err)
                    .await;
                Err(RunError::Remote {
                    message,
                    source: err,
                })
            }
        }
    }

    /// Mounts the configured cache volume and verifies the mount source.
    ///
    /// The script resolves the disk by its `volume-<uuid>` serial, refuses the
    /// root disk, requires an ext4 signature, refuses a different source at the
    /// path, mounts the device and confirms with `findmnt` that the path's
    /// source is that device. Any other outcome fails the run and tears the
    /// instance down: a configured cache is required, not best-effort.
    ///
    /// When `create_cache_directories` is enabled, the cache subdirectories are
    /// created after verification so language toolchains can write
    /// immediately. The `mountpoint -q` guard remains as a second check.
    async fn mount_cache_volume(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        volume_id: &str,
    ) -> Result<(), RunError<B::Error>> {
        let config = self.syncer.config();
        let mount_path = config.volume_mount_path.clone();
        let script = cache_device::mount_script(volume_id, &mount_path, &config.ssh_user);

        let output = match self.syncer.run_remote_raw(networking, &script) {
            Ok(output) => output,
            Err(err) => {
                let message = self
                    .teardown_with_note(handle, Some(volume_id), Some(networking), &err)
                    .await;
                return Err(RunError::CacheMount {
                    message,
                    source: Some(err),
                });
            }
        };

        let report = match cache_device::parse_report(&output.stdout) {
            Ok(report) => report,
            Err(error) => {
                let failure = format!("cache mount script produced no usable status: {error}");
                let message = self
                    .teardown_with_note(handle, Some(volume_id), Some(networking), &failure)
                    .await;
                return Err(RunError::CacheMount {
                    message,
                    source: None,
                });
            }
        };

        if report.status() != &CacheDeviceStatus::Verified {
            let failure = format!(
                "cache volume {volume_id} was not verified: {}",
                report.describe()
            );
            let message = self
                .teardown_with_note(handle, Some(volume_id), Some(networking), &failure)
                .await;
            return Err(RunError::CacheMount {
                message,
                source: None,
            });
        }

        if config.create_cache_directories {
            let command = format!(
                "if mountpoint -q {path} 2>/dev/null; then {create}; fi",
                path = escape(mount_path.as_str().into()),
                create = create_cache_directories_command(&mount_path)
            );
            if let Err(err) = self.syncer.run_remote_raw(networking, &command) {
                let message = self
                    .teardown_with_note(handle, Some(volume_id), Some(networking), &err)
                    .await;
                return Err(RunError::CacheMount {
                    message,
                    source: Some(err),
                });
            }
        }

        Ok(())
    }

    /// Polls for cloud-init's `boot-finished` marker until it appears or the
    /// timeout elapses.
    ///
    /// A failure to run the check, or a timeout, destroys the instance.
    ///
    /// For example, the marker `/var/lib/cloud/instance/boot-finished`
    /// appearing ends the wait successfully, while the deadline passing
    /// destroys the instance and reports the timeout.
    async fn wait_for_cloud_init(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        volume_id: Option<&str>,
    ) -> Result<(), RunError<B::Error>> {
        let deadline = Instant::now() + self.cloud_init_wait_timeout;
        let cloud_init_finished_marker = "/var/lib/cloud/instance/boot-finished";
        let command = format!("sudo test -f {cloud_init_finished_marker}");

        while Instant::now() <= deadline {
            let finished = match self.syncer.run_remote(networking, &command) {
                Ok(output) => matches!(output.exit_code, Some(0)),
                Err(err) => {
                    let message = self
                        .teardown_with_note(handle, volume_id, Some(networking), &err)
                        .await;
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
        let message_with_teardown = self
            .teardown_with_note(handle, volume_id, Some(networking), &timeout_message)
            .await;
        Err(RunError::ProvisioningTimeout {
            message: message_with_teardown,
        })
    }

    /// Unmounts the cache path when a volume was configured.
    ///
    /// Returns a description of the failure, if any. The caller still requests
    /// the power-off fallback, which teardown performs; this note keeps the
    /// unmount outcome visible when teardown also fails.
    async fn unmount_cache(
        &self,
        networking: &InstanceNetworking,
        volume_id: Option<&str>,
    ) -> Option<String> {
        let volume_id = volume_id?;
        let mount_path = &self.syncer.config().volume_mount_path;
        let script = cache_device::unmount_script(mount_path);

        match self.syncer.run_remote_raw(networking, &script) {
            Ok(output) => match cache_device::parse_report(&output.stdout) {
                Ok(report) if report.status() == &CacheDeviceStatus::Unmounted => None,
                Ok(report) => Some(format!(
                    "unmount of {mount_path} for volume {volume_id} reported {}",
                    report.describe()
                )),
                Err(error) => Some(format!(
                    "unmount of {mount_path} for volume {volume_id} produced no usable status: {error}"
                )),
            },
            Err(error) => Some(format!(
                "unmount request for {mount_path} (volume {volume_id}) failed: {error}"
            )),
        }
    }

    /// Tears the instance down and returns `err` as text, with a note appended
    /// when the teardown also fails.
    ///
    /// A configured cache is flushed and unmounted first; teardown then stops
    /// the server, detaches the volume, waits for it to become `available`, and
    /// only then deletes the server.
    ///
    /// For example, `teardown` failing yields `"<err> (teardown also failed:
    /// ...)"` rather than discarding the original error.
    async fn teardown_with_note<E: Display>(
        &self,
        handle: &InstanceHandle,
        volume_id: Option<&str>,
        networking: Option<&InstanceNetworking>,
        err: &E,
    ) -> String {
        let mut message = err.to_string();
        if let Some(networking) = networking
            && let Some(note) = self.unmount_cache(networking, volume_id).await
        {
            message = format!("{message} ({note})");
        }

        let teardown_error = self.backend.teardown(handle, volume_id).await.err();
        append_teardown_note(message, teardown_error.as_ref())
    }
}
