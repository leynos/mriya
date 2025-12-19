//! BDD scenarios for the run workflow.

use rstest_bdd_macros::scenario;

use super::test_helpers::{RunContext, run_context};

#[scenario(
    path = "tests/features/run.feature",
    name = "Propagate remote exit codes through the CLI orchestrator"
)]
fn scenario_propagate_exit_codes(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Surface sync failures and still teardown"
)]
fn scenario_surface_sync_failures(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Surface teardown failure after success"
)]
fn scenario_surface_teardown_failures(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Mount cache volume before syncing when volume ID is configured"
)]
fn scenario_mount_cache_volume(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Continue execution when volume mount fails"
)]
fn scenario_volume_mount_failure_continues(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Route Cargo caches to the mounted cache volume"
)]
fn scenario_route_cargo_caches(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Allow disabling cache routing"
)]
fn scenario_disable_cache_routing(run_context: RunContext) {
    let _ = run_context;
}
