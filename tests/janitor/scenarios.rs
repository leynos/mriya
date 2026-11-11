//! BDD scenarios for the janitor sweep.

use rstest_bdd_macros::scenario;

use super::test_helpers::{JanitorContext, janitor_context};

#[scenario(
    path = "tests/features/janitor.feature",
    name = "Delete tagged resources and verify clean state"
)]
fn scenario_delete_tagged_resources(janitor_context: JanitorContext) {
    let _ = janitor_context;
}

#[scenario(
    path = "tests/features/janitor.feature",
    name = "Fail the sweep when resources remain"
)]
fn scenario_fail_when_not_clean(janitor_context: JanitorContext) {
    let _ = janitor_context;
}

#[scenario(
    path = "tests/features/janitor.feature",
    name = "Reject configuration with a blank project"
)]
fn scenario_reject_blank_project(janitor_context: JanitorContext) {
    let _ = janitor_context;
}

#[scenario(
    path = "tests/features/janitor.feature",
    name = "Reject configuration with a blank test run"
)]
fn scenario_reject_blank_test_run(janitor_context: JanitorContext) {
    let _ = janitor_context;
}
