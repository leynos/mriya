//! Cloud-init readiness polling for the run orchestrator.
//!
//! The wait is bounded by a deadline: no probe starts after it, and each probe
//! is wrapped in a remote `timeout` so one answer cannot outlive the window.

use std::{
    fmt::Display,
    time::{Duration, Instant},
};

use tokio::time::sleep;

use super::{RunOrchestrator, StepFailure, check_cancelled, error::RunError};
use crate::{
    backend::{Backend, InstanceNetworking},
    cancel::Cancellation,
    sync::CommandRunner,
};

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
{
    /// Polls for cloud-init's `boot-finished` marker until it appears or the
    /// timeout elapses.
    pub(super) async fn wait_for_cloud_init(
        &self,
        networking: &InstanceNetworking,
        cancellation: &Cancellation,
    ) -> Result<(), StepFailure<B::Error>> {
        let deadline = Instant::now() + self.cloud_init_wait_timeout;
        let cloud_init_finished_marker = "/var/lib/cloud/instance/boot-finished";
        let command = format!("sudo test -f {cloud_init_finished_marker}");

        while Instant::now() <= deadline {
            check_cancelled(cancellation)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if self.probe_cloud_init(networking, &command, remaining)? {
                return Ok(());
            }

            let now = Instant::now();
            if now > deadline {
                break;
            }
            // Never sleep past the deadline: a two-second interval with a
            // one-second budget must not extend the wait to two seconds.
            tokio::select! {
                () = sleep(self.cloud_init_poll_interval.min(deadline - now)) => {}
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

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
{
    /// Runs one cloud-init probe bounded by the wait's remaining budget.
    ///
    /// The probe is a blocking SSH call, so a bound on the following sleep
    /// alone would let a probe that started inside the window answer long after
    /// it closed. Wrapping the remote command in `timeout` bounds the provider
    /// round trip; a probe that is cut off did not observe the marker, so the
    /// next pass of the wait sees the deadline and reports the timeout.
    fn probe_cloud_init(
        &self,
        networking: &InstanceNetworking,
        command: &str,
        remaining: Duration,
    ) -> Result<bool, StepFailure<B::Error>> {
        let seconds = remaining.as_secs().max(1);
        let bounded = format!("timeout {seconds} {command}");
        match self.syncer.run_remote(networking, &bounded) {
            Ok(output) => Ok(matches!(output.exit_code, Some(0))),
            Err(error) => Err(StepFailure::Error(RunError::Provisioning {
                message: error.to_string(),
                source: error,
            })),
        }
    }
}
