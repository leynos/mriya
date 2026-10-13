//! Behavioural tests for the `mriya run` CLI.
//!
//! The shared binary helper enables fake run modes through the
//! `test-backdoors` feature.

use predicates::str::contains;

#[path = "common/test_binary.rs"]
mod test_binary;

#[test]
fn cli_run_propagates_exit_code_and_streams_output() -> Result<(), &'static test_binary::BuildError>
{
    let mut cmd = test_binary::mriya_cmd()?;
    cmd.env("MRIYA_FAKE_RUN_ENABLE", "1");
    cmd.env("MRIYA_FAKE_RUN_MODE", "exit-7");
    cmd.args(["run", "--", "echo", "ok"]);

    cmd.assert()
        .code(7)
        .stdout(contains("fake-stdout"))
        .stderr(contains("fake-stderr"));
    Ok(())
}

#[test]
fn cli_run_reports_missing_exit_code() -> Result<(), &'static test_binary::BuildError> {
    let mut cmd = test_binary::mriya_cmd()?;
    cmd.env("MRIYA_FAKE_RUN_ENABLE", "1");
    cmd.env("MRIYA_FAKE_RUN_MODE", "missing-exit");
    cmd.args(["run", "--", "echo", "ok"]);

    cmd.assert()
        .failure()
        .code(1)
        .stderr(contains("remote command terminated without an exit status"));
    Ok(())
}
