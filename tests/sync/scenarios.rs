//! BDD scenario definitions for sync module.
//!
//! Declares Gherkin-style scenarios that bind to step definitions in
//! `bdd_steps` and fixtures from `test_helpers`. Fallible fixtures are passed
//! to the steps as `Result` values so setup failures surface as step errors.

use rstest_bdd_macros::scenario;

use super::test_helpers::{
    ScriptedContextResult,
    WorkspaceResult,
    error,
    output,
    scripted_context_result,
    workspace_result,
};

#[scenario(
    path = "tests/features/sync.feature",
    name = "Preserve gitignored caches on the remote"
)]
fn scenario_preserve_caches(workspace_result: WorkspaceResult) { drop(workspace_result); }

#[scenario(
    path = "tests/features/sync.feature",
    name = "Propagate remote exit codes"
)]
fn scenario_propagate_exit_codes(
    scripted_context_result: ScriptedContextResult,
    output: mriya::sync::RemoteCommandOutput,
) {
    drop((scripted_context_result, output));
}

#[scenario(path = "tests/features/sync.feature", name = "Surface sync failures")]
fn scenario_surface_failures(
    scripted_context_result: ScriptedContextResult,
    error: mriya::sync::SyncError,
) {
    drop((scripted_context_result, error));
}
