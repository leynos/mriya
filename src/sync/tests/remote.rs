//! Tests for remote command wrapping and cache routing.

use super::super::*;
use crate::backend::InstanceNetworking;
use crate::test_support::ScriptedRunner;
use rstest::{fixture, rstest};
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr};

#[fixture]
fn base_config() -> SyncConfig {
    SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: String::from("/remote/path"),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
        ssh_identity_file: Some(String::from("~/.ssh/id_ed25519")),
        volume_mount_path: String::from("/mriya"),
        route_build_caches: true,
    }
}

#[fixture]
fn networking() -> InstanceNetworking {
    InstanceNetworking {
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        ssh_port: 2222,
    }
}

fn run_remote_with_fake_output(
    cfg: SyncConfig,
    networking: &InstanceNetworking,
    script: impl Fn(&ScriptedRunner),
) -> Result<RemoteCommandOutput, SyncError> {
    let runner = ScriptedRunner::new();
    script(&runner);
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    syncer.run_remote(networking, "echo ok")
}

#[rstest]
fn run_remote_returns_missing_exit_code(base_config: SyncConfig, networking: InstanceNetworking) {
    let output = run_remote_with_fake_output(base_config, &networking, |runner| {
        runner.push_missing_exit_code();
    })
    .expect("missing exit code should be propagated as None");
    assert!(output.exit_code.is_none());
}

#[rstest]
fn run_remote_propagates_exit_code(base_config: SyncConfig, networking: InstanceNetworking) {
    let output = run_remote_with_fake_output(base_config, &networking, |runner| {
        runner.push_exit_code(7);
    })
    .unwrap_or_else(|err| panic!("run_remote should succeed: {err}"));
    assert_eq!(output.exit_code, Some(7));
    assert_eq!(output.stdout, "");
}

#[rstest]
fn run_remote_cd_prefixes_remote_path(base_config: SyncConfig, networking: InstanceNetworking) {
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(base_config, runner).expect("config should validate");
    let _ = syncer
        .run_remote(&networking, "cargo test")
        .expect("run_remote should succeed");

    let args = syncer.build_remote_command("cargo test");
    assert!(
        args.contains("cd /remote/path && cargo test"),
        "remote command should change directory, got: {args}"
    );
}

#[rstest]
fn build_remote_command_routes_cargo_caches_when_volume_is_mounted(base_config: SyncConfig) {
    let cfg = SyncConfig {
        volume_mount_path: String::from("/mriya"),
        route_build_caches: true,
        ..base_config
    };
    let runner = ScriptedRunner::new();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");

    let command = syncer.build_remote_command("cargo test");
    assert!(
        command.contains("mountpoint -q /mriya"),
        "expected mountpoint guard, got: {command}"
    );
    assert!(
        command.contains("export CARGO_HOME=/mriya/cargo"),
        "expected CARGO_HOME export, got: {command}"
    );
    assert!(
        command.contains("export RUSTUP_HOME=/mriya/rustup"),
        "expected RUSTUP_HOME export, got: {command}"
    );
    assert!(
        command.contains("export CARGO_TARGET_DIR=/mriya/target"),
        "expected CARGO_TARGET_DIR export, got: {command}"
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
    assert!(
        !command.contains("CARGO_HOME="),
        "expected cache routing to be disabled, got: {command}"
    );
}

#[rstest]
fn build_remote_command_escapes_volume_mount_path_with_spaces(base_config: SyncConfig) {
    let cfg = SyncConfig {
        volume_mount_path: String::from("/mnt/mriya cache"),
        route_build_caches: true,
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
    let cfg = base_config;
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let wrapped = syncer.build_remote_command("echo ok");
    let args = syncer.build_ssh_args(&networking, &wrapped);

    assert_eq!(
        args.last(),
        Some(&OsString::from(wrapped.clone())),
        "ssh args should forward the already wrapped remote command"
    );
}
