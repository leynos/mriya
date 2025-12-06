//! Behavioural smoke test for the CLI entrypoint.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use predicates::str::contains;

#[test]
fn cli_displays_help() {
    let mut cmd = cargo_bin_cmd!("mriya");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(contains("run").and(contains("Provision")));
}
