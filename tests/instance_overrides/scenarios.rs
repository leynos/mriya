//! BDD scenarios for instance type and image overrides on `mriya run`.

use rstest_bdd_macros::scenario;

use super::test_helpers::{CliContext, cli_context};

#[scenario(
    path = "tests/features/instance_overrides.feature",
    name = "Use configured defaults when overrides are omitted"
)]
fn scenario_defaults_apply(cli_context: CliContext) {
    let _ = cli_context;
}

#[scenario(
    path = "tests/features/instance_overrides.feature",
    name = "Override instance type and image per run"
)]
fn scenario_overrides_apply(cli_context: CliContext) {
    let _ = cli_context;
}

#[scenario(
    path = "tests/features/instance_overrides.feature",
    name = "Reject empty instance type override"
)]
fn scenario_reject_empty_instance_type(cli_context: CliContext) {
    let _ = cli_context;
}

#[scenario(
    path = "tests/features/instance_overrides.feature",
    name = "Reject empty image override"
)]
fn scenario_reject_empty_image(cli_context: CliContext) {
    let _ = cli_context;
}
