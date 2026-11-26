//! Orchestrates end-to-end remote runs over SSH.
//!
//! The run workflow provisions an instance via a backend, waits for SSH
//! readiness, synchronizes the local workspace, executes a remote command
//! using the system `ssh` client, and tears the instance down. Every exit —
//! success, remote failure, readiness failure, provisioning failure,
//! cancellation — passes through the same ownership-aware cleanup, which works
//! from the run's resource ledger rather than from a single instance handle.

use std::{fmt::Display, time::Duration};

use camino::Utf8Path;
use shell_escape::unix::escape;
use tokio::time::{Instant, sleep};

mod error;
mod finish;

pub use error::{RunCancellation, RunCleanupFailure, RunError, RunOutcome};
use finish::RunCompletion;

use crate::{
    backend::{Backend, InstanceNetworking, InstanceRequest},
    cancel::{Cancellation, until_cancelled},
    cleanup::{CleanupFailure, CleanupReport},
    ownership::ResourceLedger,
    sync::{CommandRunner, RemoteCommandOutput, Syncer, create_cache_directories_command},
};

/// Delay between checks for the cloud-init completion marker.
const CLOUD_INIT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Total time to wait for cloud-init to finish (ten minutes).
const CLOUD_INIT_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

/// Why the step sequence stopped short of a completed remote command.
enum StepFailure<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// A step failed; cleanup still runs.
    Error(RunError<BackendError>),
    /// A signal interrupted a step or arrived between steps.
    Cancelled,
}

/// How ownership-aware cleanup ended.
enum CleanupOutcome {
    /// Cleanup verified every postcondition.
    Completed(CleanupReport),
    /// Cleanup ran but could not verify its postconditions.
    Failed(CleanupFailure),
    /// A second signal stopped cleanup before it finished.
    Interrupted,
}

/// Inputs shared by every step of one run.
#[derive(Clone, Copy)]
pub struct RunInputs<'a> {
    /// The request that describes the instance to provision.
    pub request: &'a InstanceRequest,
    /// The run's ownership ledger.
    pub ledger: &'a ResourceLedger,
    /// Local workspace to synchronize.
    pub source: &'a Utf8Path,
    /// Command to execute on the remote host.
    pub remote_command: &'a str,
    /// Cancellation view shared with the signal listener.
    pub cancellation: &'a Cancellation,
}

/// Executes the remote run flow using the provided backend and syncer.
#[derive(Debug)]
pub struct RunOrchestrator<B, R: CommandRunner> {
    /// Backend that creates, waits for, and cleans up the instance.
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

    /// Runs the end-to-end workflow and returns the remote command's result.
    ///
    /// The remote exit code is returned even when non-zero. Ownership-aware
    /// cleanup always runs; a cleanup failure is reported with the primary
    /// error, the remote status, and the residual resource identifiers.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] when provisioning, readiness checks,
    /// synchronization, remote execution, cleanup, or cancellation must be
    /// reported.
    pub async fn execute(&self, inputs: RunInputs<'_>) -> Result<RunOutcome, RunError<B::Error>> {
        let mut networking: Option<InstanceNetworking> = None;
        let step = self.run_steps(inputs, &mut networking).await;
        let warnings = self.release_cache(inputs.request, networking.as_ref());

        match step {
            Ok(output) => {
                self.finish_success(
                    inputs.ledger,
                    RunCompletion { output, warnings },
                    inputs.cancellation,
                )
                .await
            }
            Err(StepFailure::Cancelled) => Err(self
                .finish_cancelled(inputs.ledger, inputs.cancellation)
                .await),
            Err(StepFailure::Error(error)) => Err(self
                .finish_failure(inputs.ledger, error, inputs.cancellation)
                .await),
        }
    }

    /// Runs the provisioning, readiness, sync, cloud-init, and remote steps.
    async fn run_steps(
        &self,
        inputs: RunInputs<'_>,
        networking: &mut Option<InstanceNetworking>,
    ) -> Result<RemoteCommandOutput, StepFailure<B::Error>> {
        let RunInputs {
            request,
            ledger,
            source,
            remote_command,
            cancellation,
        } = inputs;
        let handle = match until_cancelled(self.backend.create(request, ledger), cancellation).await
        {
            None => return Err(StepFailure::Cancelled),
            Some(Err(error)) => return Err(StepFailure::Error(RunError::Provision(error))),
            Some(Ok(handle)) => handle,
        };

        let ready = match until_cancelled(self.backend.wait_for_ready(&handle), cancellation).await
        {
            None => return Err(StepFailure::Cancelled),
            Some(Err(error)) => {
                return Err(StepFailure::Error(RunError::Wait {
                    message: error.to_string(),
                    source: error,
                }));
            }
            Some(Ok(ready)) => ready,
        };
        *networking = Some(ready.clone());

        if request.volume_id.is_some() {
            check_cancelled(cancellation)?;
            self.mount_cache_volume(&ready)
                .map_err(StepFailure::Error)?;
        }

        check_cancelled(cancellation)?;
        let destination = self.syncer.destination_for(&ready);
        self.syncer.sync(source, &destination).map_err(|error| {
            StepFailure::Error(RunError::Sync {
                message: error.to_string(),
                source: error,
            })
        })?;

        if request.cloud_init_user_data.is_some() {
            self.wait_for_cloud_init(&ready, cancellation).await?;
        }

        check_cancelled(cancellation)?;
        let output = self
            .syncer
            .run_remote(&ready, remote_command)
            .map_err(|error| {
                StepFailure::Error(RunError::Remote {
                    message: error.to_string(),
                    source: error,
                })
            })?;

        // A signal that arrived while the blocking SSH call ran killed the
        // child; the command's own status is then meaningless, and the run is
        // reported as cancelled rather than as a remote failure.
        check_cancelled(cancellation)?;
        Ok(output)
    }

    /// Releases the cache volume before cleanup so the guest stops writing.
    ///
    /// Failures are reported as warnings: the volume is detached by cleanup
    /// next, and a failed unmount must not stop that.
    fn release_cache(
        &self,
        request: &InstanceRequest,
        networking: Option<&InstanceNetworking>,
    ) -> Vec<String> {
        if request.volume_id.is_none() {
            return Vec::new();
        }
        let Some(ready) = networking else {
            return Vec::new();
        };

        let mount_path = &self.syncer.config().volume_mount_path;
        let escaped = escape(mount_path.as_str().into());
        let command = format!("sync; if mountpoint -q {escaped}; then sudo umount {escaped}; fi");
        match self.syncer.run_remote_raw(ready, &command) {
            Ok(output) if output.exit_code == Some(0) => Vec::new(),
            Ok(output) => vec![format!(
                "could not release the cache volume at {mount_path}: {detail}",
                detail = remote_status_detail(&output)
            )],
            Err(error) => vec![format!(
                "could not release the cache volume at {mount_path}: {error}"
            )],
        }
    }

    /// Mounts the cache volume via SSH and creates cache subdirectories.
    ///
    /// The mount command is idempotent: it creates the mount point directory
    /// and attempts to mount `/dev/vdb`. The mount itself is best-effort, so a
    /// first-time unformatted volume does not fail the run.
    fn mount_cache_volume(
        &self,
        networking: &InstanceNetworking,
    ) -> Result<(), RunError<B::Error>> {
        let config = self.syncer.config();
        let mount_path = &config.volume_mount_path;
        let escaped_mount_path = escape(mount_path.as_str().into());

        let mkdir_cache_dirs = if config.create_cache_directories {
            format!(
                " && if mountpoint -q {path} 2>/dev/null; then {create_dirs}; fi",
                path = escaped_mount_path,
                create_dirs = create_cache_directories_command(mount_path)
            )
        } else {
            String::new()
        };

        let mount_command = format!(
            concat!(
                "sudo mkdir -p {path} && ",
                "sudo mount /dev/vdb {path} 2>/dev/null || true"
            ),
            path = escaped_mount_path
        );
        let full_command = format!("{mount_command}{mkdir_cache_dirs}");

        self.syncer
            .run_remote_raw(networking, &full_command)
            .map(|_| ())
            .map_err(|error| RunError::Sync {
                message: error.to_string(),
                source: error,
            })
    }

    /// Polls for cloud-init's `boot-finished` marker until it appears or the
    /// timeout elapses.
    async fn wait_for_cloud_init(
        &self,
        networking: &InstanceNetworking,
        cancellation: &Cancellation,
    ) -> Result<(), StepFailure<B::Error>> {
        let deadline = Instant::now() + self.cloud_init_wait_timeout;
        let cloud_init_finished_marker = "/var/lib/cloud/instance/boot-finished";
        let command = format!("sudo test -f {cloud_init_finished_marker}");

        while Instant::now() <= deadline {
            check_cancelled(cancellation)?;
            let finished = match self.syncer.run_remote(networking, &command) {
                Ok(output) => matches!(output.exit_code, Some(0)),
                Err(error) => {
                    return Err(StepFailure::Error(RunError::Provisioning {
                        message: error.to_string(),
                        source: error,
                    }));
                }
            };
            if finished {
                return Ok(());
            }

            tokio::select! {
                () = sleep(self.cloud_init_poll_interval) => {}
                _signal = cancellation.cancelled() => {
                    return Err(StepFailure::Cancelled);
                }
            }
        }

        Err(StepFailure::Error(RunError::ProvisioningTimeout {
            message: format!(
                "cloud-init did not finish within {} seconds",
                self.cloud_init_wait_timeout.as_secs()
            ),
        }))
    }
}

/// Returns `Cancelled` when a signal has already arrived.
fn check_cancelled<BackendError>(
    cancellation: &Cancellation,
) -> Result<(), StepFailure<BackendError>>
where
    BackendError: std::error::Error + 'static,
{
    if cancellation.is_cancelled() {
        return Err(StepFailure::Cancelled);
    }
    Ok(())
}

/// Describes a non-success remote status, including trimmed stderr when there
/// is any.
fn remote_status_detail(output: &RemoteCommandOutput) -> String {
    let stderr = output.stderr.trim();
    match (output.exit_code, stderr.is_empty()) {
        (Some(code), true) => format!("remote command exited with status {code}"),
        (Some(code), false) => format!("remote command exited with status {code}: {stderr}"),
        (None, true) => String::from("remote command terminated without an exit status"),
        (None, false) => {
            format!("remote command terminated without an exit status: {stderr}")
        }
    }
}
