//! Behavioural smoke test for the CLI entrypoint.

use assert_cmd::cargo::cargo_bin_cmd;

#[test]
fn cli_exits_successfully_without_output() {
    let mut cmd = cargo_bin_cmd!("mriya");
    cmd.assert().success().stdout("").stderr("");
}
