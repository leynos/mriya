//! Tests for remote command wrapping and cache routing.

use super::super::*;
use crate::backend::InstanceNetworking;
use crate::test_support::ScriptedRunner;
use rstest::rstest;
use std::ffi::OsString;

use super::fixtures::{base_config, networking};

const CACHE_ROUTING_VARS: &[&str] = &[
    "CARGO_HOME=",
    "RUSTUP_HOME=",
    "CARGO_TARGET_DIR=",
    "GOMODCACHE=",
    "GOCACHE=",
    "PIP_CACHE_DIR=",
    "npm_config_cache=",
    "YARN_CACHE_FOLDER=",
    "PNPM_STORE_PATH=",
];

fn run_remote_with_fake_output(
    cfg: SyncConfig,
    networking: &InstanceNetworking,
    script: impl Fn(&ScriptedRunner),
) -> Result<(ScriptedRunner, RemoteCommandOutput), SyncError> {
    let runner = ScriptedRunner::new();
    script(&runner);
    let syncer = Syncer::new(cfg, runner.clone()).expect("config should validate");
    let output = syncer.run_remote(networking, "echo ok")?;
    Ok((runner, output))
}

#[rstest]
#[case(None, "")]
#[case(Some(7), "")]
fn run_remote_propagates_exit_codes(
    base_config: SyncConfig,
    networking: InstanceNetworking,
    #[case] exit_code: Option<i32>,
    #[case] expected_stdout: &str,
) {
    let (runner, output) =
        run_remote_with_fake_output(base_config, &networking, |runner| match exit_code {
            None => runner.push_missing_exit_code(),
            Some(code) => runner.push_exit_code(code),
        })
        .expect("run_remote should succeed regardless of exit code presence");

    assert_eq!(output.exit_code, exit_code);
    assert_eq!(output.stdout, expected_stdout);

    let invocations = runner.invocations();
    assert_eq!(invocations.len(), 1, "expected a single ssh invocation");
    let invocation = invocations
        .first()
        .expect("expected a single invocation to exist");
    let command = invocation.command_string();
    assert!(
        command.contains("cd /remote/path && echo ok"),
        "expected remote command to change directory, got: {command}"
    );
    for fragment in ["mountpoint -q /mriya", "export CARGO_HOME=/mriya/cargo"] {
        assert!(
            command.contains(fragment),
            "expected invocation to include '{fragment}', got: {command}"
        );
    }
}

#[rstest]
fn run_remote_cd_prefixes_remote_path(base_config: SyncConfig) {
    let cfg = SyncConfig {
        route_build_caches: false,
        ..base_config
    };
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(cfg, runner.clone()).expect("config should validate");
    let _ = syncer
        .run_remote(&networking(), "cargo test")
        .expect("run_remote should succeed");

    let invocations = runner.invocations();
    assert_eq!(invocations.len(), 1, "expected a single ssh invocation");
    let invocation = invocations
        .first()
        .expect("expected a single invocation to exist");
    let command = invocation.command_string();
    assert!(
        command.contains("cd /remote/path && cargo test"),
        "expected remote command to change directory, got: {command}"
    );
    for var in CACHE_ROUTING_VARS {
        assert!(
            !command.contains(var),
            "expected invocation to avoid cache routing var '{var}', got: {command}"
        );
    }
}

#[rstest]
fn build_remote_command_routes_cargo_caches_when_volume_is_mounted(base_config: SyncConfig) {
    let runner = ScriptedRunner::new();
    let syncer = Syncer::new(base_config, runner).expect("config should validate");

    let command = syncer.build_remote_command("cargo test");
    assert!(
        command.contains("mountpoint -q /mriya"),
        "expected mountpoint guard, got: {command}"
    );
    for required in [
        "export CARGO_HOME=/mriya/cargo",
        "export RUSTUP_HOME=/mriya/rustup",
        "export CARGO_TARGET_DIR=/mriya/target",
        "export GOMODCACHE=/mriya/go/pkg/mod",
        "export GOCACHE=/mriya/go/build-cache",
        "export PIP_CACHE_DIR=/mriya/pip/cache",
        "export npm_config_cache=/mriya/npm/cache",
        "export YARN_CACHE_FOLDER=/mriya/yarn/cache",
        "export PNPM_STORE_PATH=/mriya/pnpm/store",
    ] {
        assert!(
            command.contains(required),
            "expected export '{required}', got: {command}"
        );
    }
}

#[rstest]
fn run_remote_invokes_ssh_with_wrapped_command(
    base_config: SyncConfig,
    networking: InstanceNetworking,
) {
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(base_config, runner.clone()).expect("config should validate");

    let remote_command = "echo ok";
    let expected_wrapped = syncer.build_remote_command(remote_command);
    let _ = syncer
        .run_remote(&networking, remote_command)
        .expect("run_remote should succeed");

    let invocations = runner.invocations();
    assert_eq!(invocations.len(), 1, "expected a single ssh invocation");
    let invocation = invocations
        .first()
        .expect("expected a single invocation to exist");
    assert_eq!(
        invocation.program, "ssh",
        "expected ssh binary invocation, got: {invocation:?}"
    );
    assert_eq!(
        invocation.args.last(),
        Some(&OsString::from(expected_wrapped.as_str())),
        "expected wrapped remote command to be passed as last argument"
    );

    let command = invocation.command_string();
    assert!(
        command.contains(&expected_wrapped),
        "expected invocation to contain the wrapped remote command, got: {command}"
    );
}

#[rstest]
fn build_remote_command_can_disable_cache_routing(base_config: SyncConfig) {
    let cfg = SyncConfig {
        route_build_caches: false,
        ..base_config
    };
    let runner = ScriptedRunner::new();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let command = syncer.build_remote_command("cargo test");
    for var in CACHE_ROUTING_VARS {
        assert!(
            !command.contains(var),
            "expected cache routing var '{var}' to be absent, got: {command}"
        );
    }
}

#[rstest]
fn build_remote_command_escapes_volume_mount_path_with_spaces(base_config: SyncConfig) {
    let cfg = SyncConfig {
        volume_mount_path: String::from("/mnt/mriya cache"),
        ..base_config
    };
    let runner = ScriptedRunner::new();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let command = syncer.build_remote_command("cargo test");
    assert!(
        command.contains("mountpoint -q '/mnt/mriya cache'"),
        "expected mount path to be shell escaped, got: {command}"
    );
    assert!(
        command.contains("export CARGO_HOME='/mnt/mriya cache/cargo'"),
        "expected export values to be shell escaped, got: {command}"
    );
}

#[rstest]
fn build_ssh_args_uses_wrapped_command_verbatim(
    base_config: SyncConfig,
    networking: InstanceNetworking,
) {
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(base_config, runner).expect("config should validate");
    let wrapped = syncer.build_remote_command("echo ok");
    let args = syncer.build_ssh_args(&networking, &wrapped);

    assert_eq!(
        args.last(),
        Some(&OsString::from(wrapped.clone())),
        "ssh args should forward the already wrapped remote command"
    );
}
