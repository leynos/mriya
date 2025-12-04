//! Validation tests for `SyncConfig` covering exit-code propagation.
//! Uses shared fixtures to avoid duplication.

use rstest::rstest;

use super::test_doubles::ScriptedRunner;
use super::test_helpers::{base_sync_config, networking};
use mriya::sync::Syncer;

#[rstest]
fn run_remote_reports_missing_exit_code(
    base_sync_config: mriya::sync::SyncConfig,
    networking: mriya::InstanceNetworking,
) {
    let runner = ScriptedRunner::new();
    runner.push_missing_exit_code();

    let syncer = Syncer::new(base_sync_config, runner).expect("config should be valid");

    let output = syncer
        .run_remote(&networking, "echo ok")
        .expect("missing exit code should now be propagated");

    assert!(output.exit_code.is_none());
}
