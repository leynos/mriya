//! Behavioural tests for the `mriya run` CLI.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::str::contains;

#[test]
fn cli_run_propagates_exit_code_and_streams_output() {
    let mut cmd = cargo_bin_cmd!("mriya");
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
    let mut cmd = cargo_bin_cmd!("mriya");
    cmd.env("MRIYA_FAKE_RUN_ENABLE", "1");
    cmd.env("MRIYA_FAKE_RUN_MODE", "missing-exit");
    cmd.args(["run", "--", "echo", "ok"]);

    cmd.assert()
        .failure()
        .code(1)
        .stderr(contains("remote command terminated without an exit status"));
}
