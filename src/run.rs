//! Orchestrates end-to-end remote runs over SSH.
//!
//! The run workflow provisions an instance via a backend, waits for SSH
//! readiness, synchronizes the local workspace, executes a remote command
//! using the system `ssh` client, and tears the instance down. Remote exit
//! codes are preserved so callers observe the same status locally.

use std::{
    fmt::Display,
    future::Future,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use shell_escape::unix::escape;
use thiserror::Error;
use tokio::time::sleep;

use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest},
    bench::{
        observe::{NoopObserver, Observation, RunObserver},
        record::PhaseKind,
    },
    sync::{
        CommandRunner,
        RemoteCommandOutput,
        SyncError,
        Syncer,
        create_cache_directories_command,
    },
    teardown::append_teardown_note,
};

/// Delay between checks for the cloud-init completion marker.
const CLOUD_INIT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Total time to wait for cloud-init to finish (ten minutes).
const CLOUD_INIT_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

/// The result of a run that also ran the optional benchmark probes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedRun {
    /// The workload's own output, exactly as [`RunOrchestrator::execute`]
    /// returns it.
    pub output: RemoteCommandOutput,
    /// Standard output of the pre-workload hook, when one was configured.
    pub pre_stdout: Option<String>,
    /// Standard output of the post-workload hook, when one was configured.
    pub post_stdout: Option<String>,
}

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
    /// Raised when teardown fails after the primary operation succeeded.
    #[error("failed to destroy instance: {0}")]
    Teardown(#[source] BackendError),
}

/// Executes the remote run flow using the provided backend and syncer.
pub struct RunOrchestrator<B, R: CommandRunner> {
    /// Backend that creates, waits for, and destroys the instance.
    backend: B,
    /// Syncer used for workspace transfer and remote command execution.
    syncer: Syncer<R>,
    /// Delay between cloud-init completion checks.
    cloud_init_poll_interval: Duration,
    /// Time allowed for cloud-init before a `ProvisioningTimeout` error.
    cloud_init_wait_timeout: Duration,
    /// Receives the phase timings of each run.
    ///
    /// Defaults to [`NoopObserver`], so a caller that does not ask for
    /// observation gets the same behaviour, the same return values, and the
    /// same phase order as before this field existed.
    observer: Box<dyn RunObserver>,
    /// Optional remote command run immediately before the workload.
    ///
    /// `None` on every ordinary run, which is why `mriya run` makes exactly the
    /// same remote calls it always did. A benchmark sets it to a probe command
    /// so the record can state what the guest actually provided.
    pre_workload_command: Option<String>,
    /// Optional remote command run immediately after the workload.
    post_workload_command: Option<String>,
}

impl<B, R> std::fmt::Debug for RunOrchestrator<B, R>
where
    B: std::fmt::Debug,
    R: CommandRunner + std::fmt::Debug,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunOrchestrator")
            .field("backend", &self.backend)
            .field("syncer", &self.syncer)
            .field("cloud_init_poll_interval", &self.cloud_init_poll_interval)
            .field("cloud_init_wait_timeout", &self.cloud_init_wait_timeout)
            .finish_non_exhaustive()
    }
}

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
{
    /// Creates a new orchestrator.
    #[must_use]
    pub fn new(backend: B, syncer: Syncer<R>) -> Self {
        Self {
            backend,
            syncer,
            cloud_init_poll_interval: CLOUD_INIT_POLL_INTERVAL,
            cloud_init_wait_timeout: CLOUD_INIT_WAIT_TIMEOUT,
            observer: Box::new(NoopObserver),
            pre_workload_command: None,
            post_workload_command: None,
        }
    }

    /// Sets a remote command to run immediately before the workload.
    ///
    /// The command's standard output is returned by
    /// [`RunOrchestrator::execute_observed`]. It is not part of the workload's
    /// own command, so a benchmark can probe the guest without changing the
    /// command it pins in the record.
    #[must_use]
    pub fn with_pre_workload_command(mut self, command: impl Into<String>) -> Self {
        self.pre_workload_command = Some(command.into());
        self
    }

    /// Sets a remote command to run immediately after the workload.
    #[must_use]
    pub fn with_post_workload_command(mut self, command: impl Into<String>) -> Self {
        self.post_workload_command = Some(command.into());
        self
    }

    /// Replaces the observer that receives this orchestrator's phase timings.
    ///
    /// The observer receives one start and one finish per phase, in the order
    /// the phases run. It cannot change what the run does: nothing the
    /// orchestrator does depends on anything it reports.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let observer = std::sync::Arc::new(RecordingObserver::default());
    /// let orchestrator = RunOrchestrator::new(backend, syncer)
    ///     .with_observer(Box::new(SharedObserver::new(observer.clone())));
    /// ```
    #[must_use]
    pub fn with_observer(mut self, observer: Box<dyn RunObserver>) -> Self {
        self.observer = observer;
        self
    }

    /// Returns the observer this orchestrator reports to.
    #[must_use]
    pub fn observer(&self) -> &dyn RunObserver { self.observer.as_ref() }

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
        self.execute_observed(request, source, remote_command)
            .await
            .map(|observed| observed.output)
    }

    /// Runs the workflow and returns the workload output with the two optional
    /// probe outputs.
    ///
    /// With neither hook configured this performs exactly the calls
    /// [`RunOrchestrator::execute`] performs and makes no extra remote calls,
    /// so an ordinary run is unchanged. A benchmark sets both hooks and reads
    /// the guest facts they report.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] for the same reasons [`RunOrchestrator::execute`]
    /// does. A hook that fails is reported as a remote failure, because a
    /// benchmark that could not measure the guest has not measured the run.
    pub async fn execute_observed(
        &self,
        request: &InstanceRequest,
        source: &Utf8Path,
        remote_command: &str,
    ) -> Result<ObservedRun, RunError<B::Error>> {
        let handle = self
            .observe_async(PhaseKind::Allocate, self.backend.create(request))
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

        let pre_stdout = self
            .run_hook_or_destroy(&handle, &networking, self.pre_workload_command.as_deref())
            .await?;

        let output = self
            .run_remote_or_destroy(&handle, &networking, remote_command)
            .await?;

        let post_stdout = self
            .run_hook_or_destroy(&handle, &networking, self.post_workload_command.as_deref())
            .await?;

        self.destroy_observed(handle).await?;

        Ok(ObservedRun {
            output,
            pre_stdout,
            post_stdout,
        })
    }

    /// Runs an optional hook command, destroying the instance if it cannot run.
    ///
    /// A hook that is not configured is not run at all: there is no empty
    /// command and no extra round trip.
    async fn run_hook_or_destroy(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        hook: Option<&str>,
    ) -> Result<Option<String>, RunError<B::Error>> {
        let Some(command) = hook else {
            return Ok(None);
        };
        match self.syncer.run_remote_raw(networking, command) {
            Ok(output) => Ok(Some(output.stdout)),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(RunError::Remote {
                    message,
                    source: err,
                })
            }
        }
    }

    /// Awaits `work` and reports it to the observer as one phase.
    ///
    /// The future is created before the phase starts, so a caller's own
    /// arguments are evaluated outside the measurement and only the provider's
    /// work is timed.
    ///
    /// # Examples
    ///
    /// With a recording observer, `observe_async(PhaseKind::Allocate, future)`
    /// yields the future's value and records exactly one finished `Allocate`
    /// phase, whatever the future resolves to.
    async fn observe_async<T, E, F>(&self, phase: PhaseKind, work: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        self.observer.observe(Observation::PhaseStarted(phase));
        let started = Instant::now();
        let result = work.await;
        self.observer.observe(Observation::PhaseFinished {
            phase,
            duration: started.elapsed(),
            succeeded: result.is_ok(),
        });
        result
    }

    /// Reports a synchronous phase to the observer.
    ///
    /// The sync path and the cache mount are blocking calls, so they are timed
    /// around the call rather than inside a future.
    fn observe_sync<T, E>(
        &self,
        phase: PhaseKind,
        work: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        self.observer.observe(Observation::PhaseStarted(phase));
        let started = Instant::now();
        let result = work();
        self.observer.observe(Observation::PhaseFinished {
            phase,
            duration: started.elapsed(),
            succeeded: result.is_ok(),
        });
        result
    }

    /// Destroys the instance as the final phase.
    ///
    /// Teardown is the phase whose success cannot be assumed: the backend's
    /// `destroy` resolves once the provider has confirmed the resource is
    /// gone, so a resolved call is a verified teardown and a rejected one is
    /// not.
    async fn destroy_observed(&self, handle: InstanceHandle) -> Result<(), RunError<B::Error>> {
        self.observe_async(PhaseKind::Teardown, self.backend.destroy(handle))
            .await
            .map_err(RunError::Teardown)
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
    ) -> Result<InstanceNetworking, RunError<B::Error>> {
        match self
            .observe_async(PhaseKind::Bootstrap, self.backend.wait_for_ready(handle))
            .await
        {
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

    /// Mounts the cache volume only when the request attached one.
    ///
    /// For example, a request carrying a `volume_id` mounts it, while one
    /// without an attached volume returns `Ok(())` without contacting the
    /// instance at all.
    async fn mount_volume_if_needed(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        request: &InstanceRequest,
    ) -> Result<(), RunError<B::Error>> {
        if request.volume_id.is_some() {
            self.observe_async(
                PhaseKind::CacheRestore,
                self.mount_cache_volume(handle, networking),
            )
            .await?;
        }
        Ok(())
    }

    /// Syncs the workspace to `dest`, destroying the instance if that fails.
    ///
    /// For example, a failed rsync returns the sync error with the teardown
    /// outcome appended, so a leaked instance is never reported as a plain
    /// sync failure.
    async fn sync_or_destroy(
        &self,
        handle: &InstanceHandle,
        source: &Utf8Path,
        dest: &crate::sync::SyncDestination,
    ) -> Result<(), RunError<B::Error>> {
        let synced = self.observe_sync(PhaseKind::Sync, || self.syncer.sync(source, dest));
        if let Err(err) = synced {
            let message = self.destroy_with_note(handle, &err).await;
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
    ) -> Result<RemoteCommandOutput, RunError<B::Error>> {
        match self.observe_sync(PhaseKind::RemoteExecution, || {
            self.syncer.run_remote(networking, remote_command)
        }) {
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

    /// Mounts the cache volume via SSH and creates cache subdirectories.
    ///
    /// The mount command is idempotent: it creates the mount point directory
    /// and attempts to mount `/dev/vdb`. The mount itself is best-effort
    /// because the command uses `|| true` for graceful degradation. Only SSH
    /// execution failures are surfaced as errors.
    ///
    /// When `create_cache_directories` is enabled in the sync configuration,
    /// the cache subdirectories are created after mounting so that language
    /// toolchains can write immediately.
    ///
    /// For example, with cache directories enabled and the path already a
    /// mount point, `/mnt/mriya-cache/npm` and its siblings are created before
    /// the remote command runs, so a toolchain usually does not have to create
    /// them itself.
    ///
    /// Creation is only *attempted*, never guaranteed. The guard
    /// `mountpoint -q` tests whether the path *is* a mount point rather than
    /// whether this mount attempt succeeded, so an earlier mount can satisfy
    /// it; when the path is not a mount point at all, nothing is attempted.
    /// The creation command itself ends in `2>/dev/null || true`, so a
    /// read-only or full filesystem also leaves no directories without failing
    /// the run. A toolchain must therefore cope with their absence.
    async fn mount_cache_volume(
        &self,
        handle: &InstanceHandle,
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

        match self.syncer.run_remote_raw(networking, &full_command) {
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
    ) -> Result<(), RunError<B::Error>> {
        // Cloud-init readiness is part of bootstrap: it is the guest still
        // becoming usable, and the protocol times it there rather than opening
        // an eighth phase for a wait that only some runs perform.
        self.observer
            .observe(Observation::PhaseStarted(PhaseKind::Bootstrap));
        let started = Instant::now();
        let outcome = self.poll_cloud_init(handle, networking).await;
        let succeeded = outcome.is_ok();
        self.observer.observe(Observation::PhaseFinished {
            phase: PhaseKind::Bootstrap,
            duration: started.elapsed(),
            succeeded,
        });
        outcome
    }

    /// Polls for cloud-init's completion marker, destroying the instance on
    /// failure or timeout.
    ///
    /// Split from [`RunOrchestrator::wait_for_cloud_init`] so the observation
    /// wraps every exit path, including the ones that destroy the instance.
    async fn poll_cloud_init(
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

    /// Destroys the instance and returns `err` as text, with a note appended
    /// when the teardown also fails.
    ///
    /// For example, `destroy` failing yields `"<err> (teardown also failed:
    /// ...)"` rather than discarding the original error.
    async fn destroy_with_note<E: Display>(&self, handle: &InstanceHandle, err: &E) -> String {
        let teardown_error = self.backend.destroy(handle.clone()).await.err();
        append_teardown_note(err.to_string(), teardown_error.as_ref())
    }
}
