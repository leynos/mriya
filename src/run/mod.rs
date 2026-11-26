//! Orchestrates end-to-end remote runs over SSH.
//!
//! The run workflow provisions an instance via a backend, waits for SSH
//! readiness, synchronizes the local workspace, executes a remote command
//! using the system `ssh` client, and tears the instance down. Remote exit
//! codes are preserved so callers observe the same status locally.

use std::{fmt::Display, time::Duration};

use camino::Utf8Path;
use thiserror::Error;

mod observation;
mod phases;

use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest},
    bench::{
        observe::{NoopObserver, RunObserver},
        record::PhaseKind,
    },
    sync::{CommandRunner, RemoteCommandOutput, SyncError, Syncer},
};

/// Delay between checks for the cloud-init completion marker.
const CLOUD_INIT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Total time to wait for cloud-init to finish (ten minutes).
const CLOUD_INIT_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

/// The inputs one run's phases need after readiness is established.
///
/// Grouped so the phase sequence takes one argument rather than five, and so a
/// future phase cannot be added without deciding what it needs.
struct RunPhase<'a> {
    /// The instance the phases act on.
    handle: &'a InstanceHandle,
    /// How to reach it.
    networking: &'a InstanceNetworking,
    /// The request that created it.
    request: &'a InstanceRequest,
    /// The workspace to synchronise.
    source: &'a Utf8Path,
    /// The command to run.
    remote_command: &'a str,
}

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
        let phase = RunPhase {
            handle: &handle,
            networking: &networking,
            request,
            source,
            remote_command,
        };
        let observed = self.prepare_and_run(&phase).await?;
        self.destroy_observed(handle).await?;
        Ok(observed)
    }

    /// Runs every phase between readiness and teardown.
    ///
    /// Split from [`RunOrchestrator::execute_observed`] so the sequence reads
    /// as one list of phases and each failure path is named in one place.
    async fn prepare_and_run(
        &self,
        phase: &RunPhase<'_>,
    ) -> Result<ObservedRun, RunError<B::Error>> {
        self.mount_volume_if_needed(phase.handle, phase.networking, phase.request)
            .await?;
        let dest = self.syncer.destination_for(phase.networking);
        self.sync_or_destroy(phase.handle, phase.source, &dest)
            .await?;
        if phase.request.cloud_init_user_data.is_some() {
            self.wait_for_cloud_init(phase.handle, phase.networking)
                .await?;
        }
        let pre_stdout = self
            .run_hook_or_destroy(
                phase.handle,
                phase.networking,
                self.pre_workload_command.as_deref(),
            )
            .await?;
        let output = self
            .run_remote_or_destroy(phase.handle, phase.networking, phase.remote_command)
            .await?;
        let post_stdout = self
            .run_hook_or_destroy(
                phase.handle,
                phase.networking,
                self.post_workload_command.as_deref(),
            )
            .await?;
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
}
