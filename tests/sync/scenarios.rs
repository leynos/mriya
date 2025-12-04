//! BDD scenario definitions for sync module.
//!
//! Declares Gherkin-style scenarios that bind to step definitions in
//! `bdd_steps` and fixtures from `test_helpers`.

use super::test_helpers::{ScriptedContext, Workspace};
use super::test_helpers::{error, output, scripted_context, workspace};
use rstest_bdd_macros::scenario;

#[scenario(
    path = "tests/features/sync.feature",
    name = "Preserve gitignored caches on the remote"
)]
fn scenario_preserve_caches(workspace: Workspace) {
    let _ = workspace;
}

#[scenario(
    path = "tests/features/sync.feature",
    name = "Propagate remote exit codes"
)]
fn scenario_propagate_exit_codes(
    scripted_context: ScriptedContext,
    output: mriya::sync::RemoteCommandOutput,
) {
    let _ = (scripted_context, output);
}

#[scenario(path = "tests/features/sync.feature", name = "Surface sync failures")]
fn scenario_surface_failures(scripted_context: ScriptedContext, error: mriya::sync::SyncError) {
    let _ = (scripted_context, error);
}
