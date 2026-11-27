//! Integration tests for the init workflow's boot reporting.
//!
//! `mriya init` resolves the helper instance's boot plan before it creates the
//! cache volume, so an impossible storage request costs nothing. These tests
//! assert that ordering through the reporter: the plan event precedes the
//! allocation event, and the cache line names the volume the run prepares.

use std::sync::Arc;

use anyhow::Result;
use mriya::{
    InitOrchestrator,
    provisioning::ProvisioningReporter,
    sync::Syncer,
    test_support::{RecordingReporter, ReportEvent, ScriptedRunner},
};
use rstest::fixture;

use super::{
    memory_config_store::MemoryConfigStore,
    test_helpers::{InitContext, build_init_context},
};

/// A context whose formatter succeeds, paired with a recording reporter.
#[fixture]
fn reporting_context() -> Result<(InitContext, RecordingReporter)> {
    let context = build_init_context().map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok((context, RecordingReporter::new()))
}

/// Runs one init workflow with `reporter` installed, when one is supplied.
fn run_init(context: &InitContext, reporter: Option<&RecordingReporter>) -> Result<()> {
    let runner = ScriptedRunner::new();
    // One response for the format command over SSH.
    runner.push_success();
    let syncer = Syncer::new(context.sync_config.clone(), runner)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let base = InitOrchestrator::new(context.backend.clone(), syncer, MemoryConfigStore::new());
    let orchestrator = match reporter {
        Some(recorder) => {
            base.with_reporter(Arc::new(recorder.clone()) as Arc<dyn ProvisioningReporter>)
        }
        None => base,
    };

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async { orchestrator.execute(&context.request).await })?;
    Ok(())
}

/// The plan is reported before the allocation, and the cache line names the
/// volume this run prepares.
#[test]
fn init_reports_the_plan_before_the_allocation() -> Result<()> {
    let (context, reporter) = reporting_context()?;

    run_init(&context, Some(&reporter))?;

    check_init_report(&context, &reporter);
    Ok(())
}

/// Checks the recorded order and the cache identifier for one init run.
fn check_init_report(context: &InitContext, reporter: &RecordingReporter) {
    assert_eq!(
        context.backend.preflight_calls(),
        1,
        "the plan is resolved by exactly one preflight call"
    );

    let events = reporter.events();
    assert_eq!(events.len(), 2, "one plan and one allocation: {events:?}");
    assert!(
        matches!(events.first(), Some(ReportEvent::Plan(_))),
        "the plan must be reported first: {events:?}"
    );
    let Some(ReportEvent::Allocation(allocation)) = events.get(1) else {
        panic!("the allocation must be reported second: {events:?}");
    };
    assert_eq!(
        allocation.cache_volume_id.as_deref(),
        Some("vol-123"),
        "the cache line names the volume this run prepares"
    );
}

/// The reporter is optional: an orchestrator built without one still runs.
#[test]
fn init_runs_without_a_reporter() -> Result<()> {
    let (context, _) = reporting_context()?;

    run_init(&context, None)
}
