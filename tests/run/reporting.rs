//! Integration tests for the run workflow's allocation reporting.
//!
//! The reporter is what an operator sees before a long run starts, so these
//! tests assert both halves of it: the boot record comes from the handle, and
//! the cache line comes from the request. A run with no cache must say `none`
//! rather than borrowing a boot value.

use std::sync::Arc;

use mriya::{
    InstanceRequest,
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
///
/// The backend is returned with the reporter so a caller can assert how many
/// preflight calls the run made.
fn run_with_reporter(
    request: &InstanceRequest,
    with_boot_record: bool,
) -> (RecordingReporter, ScriptedBackend) {
    let context = build_run_context().expect("the run fixture should build");
    let reporter = RecordingReporter::new();
    let runner = ScriptedRunner::new();
    // Responses in order: the cache mount (only when a volume is configured),
    // the rsync, and the remote command. Extra queued responses are harmless.
    for _ in 0..4 {
        runner.push_success();
    }
    let syncer = Syncer::new(context.sync_config.clone(), runner).expect("syncer should build");
    let backend = context.backend.clone();
    if with_boot_record {
        backend.record_boot_volume();
    }
    let orchestrator: RunOrchestrator<ScriptedBackend, ScriptedRunner> =
        RunOrchestrator::new(backend.clone(), syncer)
            .with_reporter(Arc::new(reporter.clone()) as Arc<dyn ProvisioningReporter>);

    let runtime = tokio::runtime::Runtime::new().expect("runtime should start");
    runtime
        .block_on(async {
            orchestrator
                .execute(request, &context.source, "echo ok")
                .await
        })
        .expect("the run should succeed");

    (reporter, backend)
}

/// The run reports the handle's boot record and the request's cache setting
/// as separate facts.
#[test]
fn run_reports_the_boot_record_and_the_cache_separately() {
    let mut request = request().expect("the base request should build");
    request.volume_id = Some(String::from("vol-cache"));

    let (reporter, _backend) = run_with_reporter(&request, false);
    let allocations = reporter.allocations();

    assert_eq!(allocations.len(), 1, "exactly one allocation is reported");
    let recorded = allocations.first().expect("one allocation was recorded");
    assert_eq!(
        recorded.boot_volume.as_deref(),
        None,
        "the scripted backend records no boot volume, so the report must say so"
    );
    assert_eq!(recorded.cache_volume_id.as_deref(), Some("vol-cache"));
}

/// A run with no cache volume reports `none` rather than leaving the line out.
#[test]
fn run_reports_no_cache_when_none_is_configured() {
    let request = request().expect("the base request should build");

    let (reporter, _backend) = run_with_reporter(&request, false);
    let allocations = reporter.allocations();

    assert_eq!(allocations.len(), 1);
    let recorded = allocations.first().expect("one allocation was recorded");
    assert_eq!(recorded.cache_volume_id, None);
}

/// A run whose handle records a root volume reports that record, and keeps it
/// distinct from the cache identifier.
///
/// Without this case, an implementation that dropped every boot allocation
/// would still pass the absent-record cases above.
#[test]
fn run_reports_a_populated_boot_record_separately_from_the_cache() {
    let mut request = request().expect("the base request should build");
    request.volume_id = Some(String::from("vol-cache"));

    let (reporter, _backend) = run_with_reporter(&request, true);

    let allocations = reporter.allocations();
    assert_eq!(allocations.len(), 1);
    let recorded = allocations.first().expect("one allocation was recorded");
    assert_eq!(
        recorded.boot_volume.as_deref(),
        Some("vol-root"),
        "the run must report the root the handle recorded"
    );
    assert_eq!(
        recorded.cache_volume_id.as_deref(),
        Some("vol-cache"),
        "the cache line must stay the request's own value"
    );
}

/// The run does not add a preflight round-trip: its create resolves the plan
/// itself, so a reporter installed for a run hears only about the instance.
#[test]
fn run_reports_no_plan_because_create_resolves_it() {
    let request = request().expect("the base request should build");

    let (reporter, backend) = run_with_reporter(&request, false);

    assert!(
        reporter.plans().is_empty(),
        "the run reports the handle, not a separate plan: {:?}",
        reporter.plans()
    );
    assert_eq!(
        backend.preflight_calls(),
        0,
        "the run must not add a preflight round-trip; create resolves the plan"
    );
}
