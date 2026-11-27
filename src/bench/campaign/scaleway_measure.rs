//! The measurement half of the Scaleway driver.
//!
//! This module runs one attempt and reports what it measured. It deliberately
//! does not build a record: turning a measurement into a record is a separate
//! job with separate rules, and keeping them apart makes each testable on its
//! own.

use std::{sync::Arc, time::Duration};

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
    sync::{StreamingCommandRunner, Syncer},
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
        let syncer =
            Syncer::new(self.sync_config.clone(), StreamingCommandRunner).map_err(|err| {
                super::ports::AttemptFailure::without_resources(format!("syncer: {err}"))
            })?;
        let observer = Arc::new(RecordingObserver::default());
        let orchestrator = RunOrchestrator::new(backend, syncer)
            .with_observer(Box::new(SharedObserver(observer.clone())))
            .with_pre_workload_command(PROBE_COMMAND)
            .with_post_workload_command(PROBE_COMMAND);

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
        let started = std::time::Instant::now();
        let attempt_output = run_bounded(
            &runtime,
            &AttemptCall {
                orchestrator: &orchestrator,
                request,
                source: &source,
                rendered: &rendered,
                planned_lifetime_nanos: attempt.planned_lifetime_nanos,
            },
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

/// Adapts an `Arc<RecordingObserver>` to the orchestrator's boxed observer.
pub(super) struct SharedObserver(Arc<RecordingObserver>);

impl RunObserver for SharedObserver {
    fn observe(&self, observation: Observation<'_>) { self.0.observe(observation); }
}

/// Runs one attempt under its declared lifetime plus a teardown grace.
///
/// The guest-side wrapper stops an overrunning workload; this outer bound
/// catches a provisioning or probe hang, which the wrapper cannot. The grace
/// lets the orchestrator's own teardown run after the workload deadline.
fn run_bounded(
    runtime: &tokio::runtime::Runtime,
    call: &AttemptCall<'_>,
    lifecycle: &Arc<RecordingSink>,
) -> Result<crate::ObservedRun, super::ports::AttemptFailure> {
    let outer_limit =
        Duration::from_nanos(call.planned_lifetime_nanos).saturating_add(TEARDOWN_GRACE);
    match runtime.block_on(async {
        tokio::time::timeout(
            outer_limit,
            call.orchestrator
                .execute_observed(call.request, call.source, call.rendered),
        )
        .await
    }) {
        Ok(result) => result.map_err(|err| super::ports::AttemptFailure {
            reason: format!("run failed: {err}"),
            resource_ids: lifecycle.created_ids(),
            allocation_uncertain: lifecycle.allocation_uncertain(),
        }),
        Err(_elapsed) => Err(super::ports::AttemptFailure {
            reason: String::from(
                "the attempt exceeded its declared lifetime before teardown completed, so its \
                 resources must be reconciled",
            ),
            resource_ids: lifecycle.created_ids(),
            allocation_uncertain: true,
        }),
    }
}

/// The inputs one bounded attempt call needs.
struct AttemptCall<'a> {
    /// The orchestrator that runs the attempt.
    orchestrator: &'a RunOrchestrator<ScalewayBackend, StreamingCommandRunner>,
    /// The instance request to create.
    request: &'a InstanceRequest,
    /// The workspace to synchronize.
    source: &'a Utf8PathBuf,
    /// The rendered command to run.
    rendered: &'a str,
    /// The attempt's declared lifetime.
    planned_lifetime_nanos: u64,
}
