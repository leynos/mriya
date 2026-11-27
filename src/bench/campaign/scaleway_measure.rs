//! The measurement half of the Scaleway driver.
//!
//! This module runs one attempt and reports what it measured. It deliberately
//! does not build a record: turning a measurement into a record is a separate
//! job with separate rules, and keeping them apart makes each testable on its
//! own.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Extra time allowed for teardown after an attempt's declared lifetime.
const TEARDOWN_GRACE: Duration = Duration::from_secs(300);

use camino::Utf8PathBuf;

use super::{
    PlannedAttempt,
    scaleway_driver::{DriverResult, ScalewayDriver},
    scaleway_facts::{bounded_command, elapsed_nanos, shell_escape, tree_pin, workload_command},
};
use crate::{
    RunOrchestrator,
    ScalewayBackend,
    backend::InstanceRequest,
    bench::{
        lifecycle::RecordingSink,
        observe::{Observation, RecordedObservation, RecordingObserver, RunObserver},
        probe::PROBE_COMMAND,
        record::TreePin,
    },
    sync::{DeadlineCommandRunner, Syncer},
};

/// Everything one attempt measured, before it is turned into a record.
pub(super) struct Measurement {
    /// The tree pin taken before the workload ran.
    pub(super) tree: TreePin,
    /// The command vector the workload ran.
    pub(super) command: Vec<String>,
    /// The attempt's wall-clock lifetime.
    pub(super) total_nanos: u64,
    /// Standard output of the pre-workload probe.
    pub(super) probe_stdout: String,
    /// The workload's own output.
    pub(super) output: crate::sync::RemoteCommandOutput,
    /// Every phase observation, in order.
    pub(super) observations: Vec<RecordedObservation>,
    /// Provider identifiers the attempt created.
    pub(super) resource_ids: Vec<String>,
}

/// Returns the current working directory as a UTF-8 path.
pub(super) fn current_directory() -> DriverResult<Utf8PathBuf> {
    let directory = std::env::current_dir().map_err(|err| format!("working directory: {err}"))?;
    Utf8PathBuf::from_path_buf(directory)
        .map_err(|bad| format!("working directory is not UTF-8: {}", bad.display()))
}

impl ScalewayDriver {
    /// Executes one attempt and returns everything it measured.
    ///
    /// Kept separate from record construction so each half reads as one job:
    /// this half runs the workload, and `run_attempt` turns the result into a
    /// record.
    pub(super) fn measure(
        &self,
        campaign_id: &str,
        attempt: &PlannedAttempt,
        request: &InstanceRequest,
    ) -> Result<Measurement, super::ports::AttemptFailure> {
        let lifecycle = Arc::new(RecordingSink::default());
        let backend = self
            .backend(campaign_id, Some(lifecycle.clone()))
            .map_err(super::ports::AttemptFailure::without_resources)?;
        let source =
            current_directory().map_err(super::ports::AttemptFailure::without_resources)?;
        // The pin is taken before the workload runs, so a checkout that moves
        // during the attempt cannot be recorded as the code that ran.
        let tree = tree_pin(&source).map_err(super::ports::AttemptFailure::without_resources)?;
        let workload = workload_command(&attempt.workload_id)
            .map_err(super::ports::AttemptFailure::without_resources)?;
        // The declared lifetime is enforced on the guest, so an overrunning
        // workload is stopped and torn down rather than billed indefinitely.
        // The wrapper is part of the recorded command vector.
        let command = bounded_command(&workload, attempt.planned_lifetime_nanos);
        let rendered = shell_escape(&command);
        let runtime = tokio::runtime::Runtime::new().map_err(|err| {
            super::ports::AttemptFailure::without_resources(format!("async runtime: {err}"))
        })?;
        // One absolute deadline covers provisioning, probes, synchronization
        // and the workload: every remote command is killed once it passes, so
        // preparation cannot buy the workload extra time. Only teardown gets
        // the grace period beyond it.
        let started = Instant::now();
        let deadline = attempt_deadline(started, attempt.planned_lifetime_nanos)?;
        let syncer = Syncer::new(
            self.sync_config.clone(),
            DeadlineCommandRunner::new(deadline),
        )
        .map_err(|err| super::ports::AttemptFailure::without_resources(format!("syncer: {err}")))?;
        let observer = Arc::new(RecordingObserver::default());
        let orchestrator = RunOrchestrator::new(backend, syncer)
            .with_observer(Box::new(SharedObserver(observer.clone())))
            .with_pre_workload_command(PROBE_COMMAND)
            .with_post_workload_command(PROBE_COMMAND);
        let attempt_output = self.bounded_output(
            campaign_id,
            run_bounded(
                &runtime,
                &AttemptCall {
                    orchestrator: &orchestrator,
                    request,
                    source: &source,
                    rendered: &rendered,
                    deadline,
                },
            ),
            &lifecycle,
        )?;
        let total_nanos = elapsed_nanos(started.elapsed());
        // The run completed and its teardown was verified, so a later failure
        // leaves nothing behind and is reported without resource identifiers.
        let probe_stdout = attempt_output.pre_stdout.ok_or_else(|| {
            super::ports::AttemptFailure::without_resources(
                "the pre-workload probe produced no output",
            )
        })?;
        // If the checkout moved while the attempt ran, the record's pin no
        // longer describes what was synchronized, so the attempt fails.
        let after = tree_pin(&source).map_err(super::ports::AttemptFailure::without_resources)?;
        if after != tree {
            return Err(super::ports::AttemptFailure {
                reason: String::from(
                    "the working tree changed while the attempt ran, so its record could not name \
                     the code that was measured",
                ),
                resource_ids: lifecycle.created_ids(),
                allocation_uncertain: lifecycle.allocation_uncertain(),
            });
        }
        Ok(Measurement {
            tree,
            command,
            total_nanos,
            probe_stdout,
            output: attempt_output.output,
            observations: observer.observations(),
            resource_ids: lifecycle.created_ids(),
        })
    }
}

/// Returns the absolute deadline an attempt must finish its work by.
fn attempt_deadline(
    started: Instant,
    lifetime_nanos: u64,
) -> Result<Instant, super::ports::AttemptFailure> {
    started
        .checked_add(Duration::from_nanos(lifetime_nanos))
        .ok_or_else(|| {
            super::ports::AttemptFailure::without_resources(
                "the attempt's declared lifetime is too long to represent",
            )
        })
}

/// Adapts an `Arc<RecordingObserver>` to the orchestrator's boxed observer.
pub(super) struct SharedObserver(Arc<RecordingObserver>);

impl RunObserver for SharedObserver {
    fn observe(&self, observation: Observation<'_>) { self.0.observe(observation); }
}

/// How a bounded attempt ended when it did not return an observed run.
enum BoundedOutcome {
    /// The attempt returned an error of its own.
    Failed(String),
    /// The attempt outlived its declared lifetime and did not tear down.
    Expired,
}

/// Runs one attempt under its declared lifetime plus a teardown grace.
///
/// The blocking runner stops every remote command at the attempt deadline, so
/// the future returns and the orchestrator tears down. This outer bound is the
/// backstop for a provider call that hangs; the grace lets the orchestrator's
/// teardown run after the workload deadline.
fn run_bounded(
    runtime: &tokio::runtime::Runtime,
    call: &AttemptCall<'_>,
) -> Result<crate::ObservedRun, BoundedOutcome> {
    // The deadline was validated when it was computed; a representation
    // overflow here is impossible in practice, and dropping the grace is safer
    // than panicking out of a measurement.
    let outer_deadline = call
        .deadline
        .checked_add(TEARDOWN_GRACE)
        .unwrap_or(call.deadline);
    let outer_limit = outer_deadline.saturating_duration_since(Instant::now());
    match runtime.block_on(async {
        tokio::time::timeout(
            outer_limit,
            call.orchestrator
                .execute_observed(call.request, call.source, call.rendered),
        )
        .await
    }) {
        Ok(result) => result.map_err(|err| BoundedOutcome::Failed(format!("run failed: {err}"))),
        Err(_elapsed) => Err(BoundedOutcome::Expired),
    }
}

/// Describes a failure that kept the attempt's own known resources.
fn attempt_failure(
    reason: impl Into<String>,
    lifecycle: &Arc<RecordingSink>,
) -> super::ports::AttemptFailure {
    super::ports::AttemptFailure {
        reason: reason.into(),
        resource_ids: lifecycle.created_ids(),
        allocation_uncertain: lifecycle.allocation_uncertain(),
    }
}

impl ScalewayDriver {
    /// Maps a bounded attempt's outcome onto a failure the harness records.
    fn bounded_output(
        &self,
        campaign_id: &str,
        outcome: Result<crate::ObservedRun, BoundedOutcome>,
        lifecycle: &Arc<RecordingSink>,
    ) -> Result<crate::ObservedRun, super::ports::AttemptFailure> {
        match outcome {
            Ok(output) => Ok(output),
            Err(BoundedOutcome::Failed(reason)) => Err(attempt_failure(reason, lifecycle)),
            Err(BoundedOutcome::Expired) => Err(self.expired_attempt(campaign_id, lifecycle)),
        }
    }

    /// Reconciles an attempt that outlived its lifetime before it failed.
    ///
    /// The orchestrator's own teardown did not run, so the campaign's tag is
    /// used to find and delete whatever the attempt created. A successful
    /// sweep verifies that nothing carrying the tag remains, which is the
    /// strongest statement available without the instance handle.
    fn expired_attempt(
        &self,
        campaign_id: &str,
        lifecycle: &Arc<RecordingSink>,
    ) -> super::ports::AttemptFailure {
        match self.sweep(campaign_id) {
            Ok(_swept) => super::ports::AttemptFailure {
                reason: String::from(
                    "the attempt exceeded its declared lifetime before teardown completed; a \
                     reconciliation sweep then removed every campaign-tagged resource and \
                     confirmed none remained",
                ),
                resource_ids: lifecycle.created_ids(),
                allocation_uncertain: false,
            },
            Err(reason) => super::ports::AttemptFailure {
                reason: format!(
                    "the attempt exceeded its declared lifetime before teardown completed, and \
                     the reconciliation sweep failed: {reason}"
                ),
                resource_ids: lifecycle.created_ids(),
                allocation_uncertain: true,
            },
        }
    }
}

/// The inputs one bounded attempt call needs.
struct AttemptCall<'a> {
    /// The orchestrator that runs the attempt.
    orchestrator: &'a RunOrchestrator<ScalewayBackend, DeadlineCommandRunner>,
    /// The instance request to create.
    request: &'a InstanceRequest,
    /// The workspace to synchronize.
    source: &'a Utf8PathBuf,
    /// The rendered command to run.
    rendered: &'a str,
    /// Absolute time the attempt's work must finish by.
    deadline: Instant,
}
