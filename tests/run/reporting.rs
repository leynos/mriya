//! Integration tests for the run workflow's allocation reporting.
//!
//! The reporter is what an operator sees before a long run starts, so these
//! tests assert both halves of it: the boot record comes from the handle, and
//! the cache line comes from the request. A run with no cache must say `none`
//! rather than borrowing a boot value.

use std::sync::Arc;

use anyhow::Result;
use mriya::{
    RunOrchestrator,
    provisioning::ProvisioningReporter,
    sync::Syncer,
    test_support::{RecordingReporter, ScriptedRunner},
};

use super::{
    test_doubles::ScriptedBackend,
    test_helpers::{build_run_context, request},
};

/// Runs one scenario to completion with a recording reporter installed.
fn run_with_reporter(request: mriya::InstanceRequest) -> Result<RecordingReporter> {
    let context = build_run_context().map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let reporter = RecordingReporter::new();
    let runner = ScriptedRunner::new();
    // Responses in order: the cache mount (only when a volume is configured),
    // the rsync, and the remote command. Extra queued responses are harmless.
    for _ in 0..4 {
        runner.push_success();
    }
    let syncer = Syncer::new(context.sync_config.clone(), runner)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let orchestrator: RunOrchestrator<ScriptedBackend, ScriptedRunner> =
        RunOrchestrator::new(context.backend.clone(), syncer)
            .with_reporter(Arc::new(reporter.clone()) as Arc<dyn ProvisioningReporter>);

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        orchestrator
            .execute(&request, &context.source, "echo ok")
            .await
    })?;

    Ok(reporter)
}

/// The run reports the handle's boot record and the request's cache setting
/// as separate facts.
#[test]
fn run_reports_the_boot_record_and_the_cache_separately() -> Result<()> {
    let mut request = request().map_err(|err| anyhow::anyhow!(err.to_string()))?;
    request.volume_id = Some(String::from("vol-cache"));

    let reporter = run_with_reporter(request)?;
    let allocations = reporter.allocations();

    assert_eq!(allocations.len(), 1, "exactly one allocation is reported");
    let reported = &allocations[0];
    assert_eq!(
        reported.boot_volume.as_deref(),
        None,
        "the scripted backend records no boot volume, so the report must say so"
    );
    assert_eq!(reported.cache_volume_id.as_deref(), Some("vol-cache"));
    Ok(())
}

/// A run with no cache volume reports `none` rather than leaving the line out.
#[test]
fn run_reports_no_cache_when_none_is_configured() -> Result<()> {
    let request = request().map_err(|err| anyhow::anyhow!(err.to_string()))?;

    let reporter = run_with_reporter(request)?;
    let allocations = reporter.allocations();

    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0].cache_volume_id, None);
    Ok(())
}

/// The run does not report a boot plan: it never calls preflight itself, so a
/// reporter installed for a run hears only about the created instance.
#[test]
fn run_reports_no_plan_because_create_resolves_it() -> Result<()> {
    let request = request().map_err(|err| anyhow::anyhow!(err.to_string()))?;

    let reporter = run_with_reporter(request)?;

    assert!(
        reporter.plans().is_empty(),
        "the run reports the handle, not a separate plan: {:?}",
        reporter.plans()
    );
    Ok(())
}
