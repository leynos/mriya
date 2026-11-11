//! BDD scenarios for cloud-init user-data configuration.

use rstest_bdd_macros::scenario;

use super::test_helpers::{CliContext, cli_context};

#[scenario(
    path = "tests/features/cloud_init.feature",
    name = "Omit cloud-init user-data by default"
)]
fn scenario_cloud_init_default(cli_context: CliContext) {
    let _ = cli_context;
}

#[scenario(
    path = "tests/features/cloud_init.feature",
    name = "Accept inline cloud-init user-data"
)]
fn scenario_cloud_init_inline(cli_context: CliContext) {
    let _ = cli_context;
}

#[scenario(
    path = "tests/features/cloud_init.feature",
    name = "Accept file-based cloud-init user-data"
)]
fn scenario_cloud_init_file(cli_context: CliContext) {
    let _ = cli_context;
}

#[scenario(
    path = "tests/features/cloud_init.feature",
    name = "Reject empty cloud-init user-data"
)]
fn scenario_cloud_init_empty(cli_context: CliContext) {
    let _ = cli_context;
}

#[scenario(
    path = "tests/features/cloud_init.feature",
    name = "Reject missing cloud-init file"
)]
fn scenario_cloud_init_missing_file(cli_context: CliContext) {
    let _ = cli_context;
}

#[scenario(
    path = "tests/features/cloud_init.feature",
    name = "Expand a tilde cloud-init file path through HOME"
)]
fn scenario_cloud_init_tilde_file(cli_context: CliContext) {
    let _ = cli_context;
}
