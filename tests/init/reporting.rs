//! Integration tests for the init workflow's boot reporting.
//!
//! `mriya init` resolves the helper instance's boot plan before it creates the
//! cache volume, so an impossible storage request costs nothing. These tests
//! assert that ordering through the reporter: a plan is reported before any
//! allocation, and the cache line names the volume the run is preparing.

use std::sync::Arc;

use anyhow::Result;
use mriya::{
    InitOrchestrator,
    provisioning::ProvisioningReporter,
    sync::Syncer,
    test_support::{RecordingReporter, ScriptedRunner},
};

use super::{
    test_doubles::MemoryConfigStore,
    test_helpers::{build_init_context, init_context_result},
};

/// Runs one init workflow with a recording reporter installed.
fn init_with_reporter() -> Result<RecordingReporter> {
    let context = init_context_result().map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let reporter = RecordingReporter::new();
    let runner = ScriptedRunner::new();
    // One response for the format command over SSH.
    runner.push_success();
    let syncer = Syncer::new(context.sync_config.clone(), runner)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let orchestrator =
        InitOrchestrator::new(context.backend.clone(), syncer, MemoryConfigStore::new())
            .with_reporter(Arc::new(reporter.clone()) as Arc<dyn ProvisioningReporter>);

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async { orchestrator.execute(&context.request).await })?;

    Ok(reporter)
}

/// Init reports the resolved plan before it reports any created instance, and
/// the cache line names the volume being prepared.
#[test]
fn init_reports_the_plan_before_the_allocation() -> Result<()> {
    let reporter = init_with_reporter()?;

    assert_eq!(reporter.plans().len(), 1, "one plan is resolved");
    let allocations = reporter.allocations();
    assert_eq!(allocations.len(), 1, "one instance is created");
    assert_eq!(
        allocations[0].boot_volume.as_deref(),
        None,
        "the scripted backend records no root volume"
    );
    assert_eq!(
        allocations[0].cache_volume_id.as_deref(),
        Some("vol-123"),
        "the cache line names the volume this run prepares"
    );
    Ok(())
}

/// The reporter is optional: an orchestrator built without one still runs.
#[test]
fn init_runs_without_a_reporter() -> Result<()> {
    let context = build_init_context().map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(context.sync_config.clone(), runner)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let orchestrator =
        InitOrchestrator::new(context.backend.clone(), syncer, MemoryConfigStore::new());

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async { orchestrator.execute(&context.request).await })?;
    Ok(())
}
