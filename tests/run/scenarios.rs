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

#[scenario(
    path = "tests/features/run.feature",
    name = "Wait for cloud-init readiness before executing the remote command"
)]
fn scenario_cloud_init_wait(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Surface cloud-init provisioning check failures and still teardown"
)]
fn scenario_cloud_init_failure(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Surface cloud-init provisioning timeout and still teardown"
)]
fn scenario_cloud_init_timeout(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Create cache subdirectories after mounting the volume"
)]
fn scenario_create_cache_subdirectories(run_context: RunContext) {
    let _ = run_context;
}

#[scenario(
    path = "tests/features/run.feature",
    name = "Allow disabling cache directory creation"
)]
fn scenario_disable_cache_directory_creation(run_context: RunContext) {
    let _ = run_context;
}
