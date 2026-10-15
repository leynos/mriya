//! Behavioural tests for the `mriya run` CLI.
//!
//! The shared binary helper enables fake run modes through the
//! `test-backdoors` feature.

use predicates::str::contains;

#[path = "common/test_binary.rs"]
mod test_binary;

/// Checks that `mriya run` preserves its subprocess exit code and both output
/// streams when the remote command returns status 7.
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

/// Checks the fallback boundary when a remote command ends without an exit
/// status: the CLI exits non-zero and reports the missing status.
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
