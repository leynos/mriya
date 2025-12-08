//! Behavioural tests for the `mriya run` CLI.
//!
//! These tests use `escargot` to build the binary with the `test-backdoors`
//! feature enabled, which allows fake run modes to be activated via
//! environment variables.

use std::sync::LazyLock;

use escargot::CargoBuild;
use predicates::str::contains;

/// Lazily builds the binary once with the test-backdoors feature enabled.
///
/// # Panics
///
/// Panics if the binary fails to build (e.g., due to compilation errors).
#[expect(
    clippy::expect_used,
    reason = "test setup requires panic on build failure"
)]
static MRIYA_BIN: LazyLock<escargot::CargoRun> = LazyLock::new(|| {
    CargoBuild::new()
        .bin("mriya")
        .features("test-backdoors")
        .run()
        .expect("failed to build mriya with test-backdoors feature")
});

/// Creates a new command for the mriya binary with the test-backdoors feature.
fn mriya_cmd() -> assert_cmd::Command {
    MRIYA_BIN.command().into()
}

#[test]
fn cli_run_propagates_exit_code_and_streams_output() {
    let mut cmd = mriya_cmd();
    cmd.env("MRIYA_FAKE_RUN_ENABLE", "1");
    cmd.env("MRIYA_FAKE_RUN_MODE", "exit-7");
    cmd.args(["run", "--", "echo", "ok"]);

    cmd.assert()
        .code(7)
        .stdout(contains("fake-stdout"))
        .stderr(contains("fake-stderr"));
}

#[test]
fn cli_run_reports_missing_exit_code() {
    let mut cmd = mriya_cmd();
    cmd.env("MRIYA_FAKE_RUN_ENABLE", "1");
    cmd.env("MRIYA_FAKE_RUN_MODE", "missing-exit");
    cmd.args(["run", "--", "echo", "ok"]);

    cmd.assert()
        .failure()
        .code(1)
        .stderr(contains("remote command terminated without an exit status"));
}
