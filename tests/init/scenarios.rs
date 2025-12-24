//! BDD scenarios for the init workflow.

use rstest_bdd_macros::scenario;

use super::test_helpers::{InitContextResult, init_context_result};

#[scenario(
    path = "tests/features/init.feature",
    name = "Prepare a cache volume and update configuration"
)]
fn scenario_prepare_volume(init_context_result: InitContextResult) {
    drop(init_context_result);
}

#[scenario(
    path = "tests/features/init.feature",
    name = "Surface formatting failures and still teardown"
)]
fn scenario_format_failure(init_context_result: InitContextResult) {
    drop(init_context_result);
}

#[scenario(
    path = "tests/features/init.feature",
    name = "Reject overwriting an existing volume ID without force"
)]
fn scenario_existing_volume_rejected(init_context_result: InitContextResult) {
    drop(init_context_result);
}

#[scenario(
    path = "tests/features/init.feature",
    name = "Allow overwriting an existing volume ID with force"
)]
fn scenario_existing_volume_forced(init_context_result: InitContextResult) {
    drop(init_context_result);
}

#[scenario(
    path = "tests/features/init.feature",
    name = "Surface volume creation failures"
)]
fn scenario_volume_create_failure(init_context_result: InitContextResult) {
    drop(init_context_result);
}

#[scenario(
    path = "tests/features/init.feature",
    name = "Surface provisioning failures"
)]
fn scenario_provision_failure(init_context_result: InitContextResult) {
    drop(init_context_result);
}

#[scenario(
    path = "tests/features/init.feature",
    name = "Surface readiness failures and still teardown"
)]
fn scenario_wait_failure(init_context_result: InitContextResult) {
    drop(init_context_result);
}

#[scenario(
    path = "tests/features/init.feature",
    name = "Surface detachment failures and still teardown"
)]
fn scenario_detach_failure(init_context_result: InitContextResult) {
    drop(init_context_result);
}

#[scenario(
    path = "tests/features/init.feature",
    name = "Surface teardown failures after success"
)]
fn scenario_teardown_failure(init_context_result: InitContextResult) {
    drop(init_context_result);
}
