//! Unit tests for the sync module.

use super::*;
use crate::backend::InstanceNetworking;
use crate::test_support::ScriptedRunner;
use rstest::{fixture, rstest};
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr};
use tempfile::TempDir;

/// Helper to assert validation rejects empty or whitespace values for a given field.
fn assert_validation_rejects_field<F>(mut cfg: SyncConfig, field_name: &str, set_field: F)
where
    F: Fn(&mut SyncConfig, String),
{
    for invalid in ["", "  "] {
        set_field(&mut cfg, invalid.to_owned());
        let Err(err) = cfg.validate() else {
            panic!("{field_name} '{invalid}' should fail");
        };
        let SyncError::InvalidConfig { ref field } = err else {
            panic!("expected InvalidConfig for {field_name}, got {err:?}");
        };
        assert_eq!(field, field_name, "expected invalid field {field_name}");
    }
}

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
    }
}

#[fixture]
fn networking() -> InstanceNetworking {
    InstanceNetworking {
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        ssh_port: 2222,
    }
}

#[rstest]
fn sync_config_validate_accepts_defaults(base_config: SyncConfig) {
    let cfg = base_config;
    assert!(cfg.validate().is_ok());
}

#[rstest]
fn sync_config_validation_rejects_rsync_bin(base_config: SyncConfig) {
    assert_validation_rejects_field(base_config, "rsync_bin", |cfg, val| cfg.rsync_bin = val);
}

#[rstest]
fn sync_config_validation_rejects_ssh_bin(base_config: SyncConfig) {
    assert_validation_rejects_field(base_config, "ssh_bin", |cfg, val| cfg.ssh_bin = val);
}

#[rstest]
fn sync_config_validation_rejects_ssh_user(base_config: SyncConfig) {
    assert_validation_rejects_field(base_config, "ssh_user", |cfg, val| cfg.ssh_user = val);
}

#[rstest]
fn sync_config_validation_rejects_remote_path(base_config: SyncConfig) {
    assert_validation_rejects_field(base_config, "remote_path", |cfg, val| cfg.remote_path = val);
}

#[rstest]
fn remote_destination_builds_expected_values(
    base_config: SyncConfig,
    networking: InstanceNetworking,
) {
    let cfg = SyncConfig {
        ssh_user: String::from("alice"),
        remote_path: String::from("/dst"),
        ..base_config
    };
    let dest = cfg.remote_destination(&networking);
    let SyncDestination::Remote {
        user,
        host,
        port,
        path,
    } = dest
    else {
        panic!("expected remote destination");
    };
    assert_eq!(user, "alice");
    assert_eq!(host, Ipv4Addr::LOCALHOST.to_string());
    assert_eq!(port, 2222);
    assert_eq!(path, Utf8PathBuf::from("/dst"));
}

#[rstest]
fn build_rsync_args_remote_includes_gitignore_filter(base_config: SyncConfig) {
    let cfg = base_config;
    let runner = ScriptedRunner::new();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let destination = SyncDestination::Remote {
        user: String::from("ubuntu"),
        host: String::from("1.2.3.4"),
        port: 2222,
        path: Utf8PathBuf::from("/remote"),
    };
    let source_dir = TempDir::new().expect("temp dir");
    let source = Utf8PathBuf::from_path_buf(source_dir.path().to_path_buf()).expect("utf8 path");
    let args = syncer
        .build_rsync_args(&source, &destination)
        .expect("args should build");

    let args_strs: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args_strs.contains(&String::from("--filter=:- .gitignore")));
    assert!(args_strs.contains(&String::from("--exclude")));
    assert!(args_strs.contains(&String::from(".git/")));
    assert!(
        args_strs.iter().any(|arg| arg.starts_with("--rsh")),
        "expected --rsh wrapper"
    );
    assert!(
        args_strs.iter().any(|arg| arg.contains("ssh -p 2222")),
        "expected ssh port in remote shell: {args_strs:?}"
    );
}

#[rstest]
fn build_rsync_args_local_omits_remote_shell(base_config: SyncConfig) {
    let cfg = base_config;
    let runner = ScriptedRunner::new();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let destination = SyncDestination::Local {
        path: Utf8PathBuf::from("/tmp/dst"),
    };
    let source_dir = TempDir::new().expect("temp dir");
    let source = Utf8PathBuf::from_path_buf(source_dir.path().to_path_buf()).expect("utf8 path");
    let args = syncer
        .build_rsync_args(&source, &destination)
        .expect("args should build");
    let args_strs: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        !args_strs.iter().any(|arg| arg.starts_with("--rsh")),
        "local sync should not set --rsh"
    );
    assert_eq!(args_strs.last().map(String::as_str), Some("/tmp/dst"));
}

#[rstest]
fn sync_returns_error_on_non_zero_rsync_status(base_config: SyncConfig) {
    let cfg = base_config;
    let runner = ScriptedRunner::new();
    runner.push_failure(12);
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let destination = SyncDestination::Local {
        path: Utf8PathBuf::from("/tmp/dst"),
    };
    let err = syncer
        .sync(Utf8Path::new("/"), &destination)
        .expect_err("non-zero rsync should error");
    let SyncError::CommandFailure {
        status,
        status_text,
        ..
    } = err
    else {
        panic!("expected CommandFailure");
    };
    assert_eq!(status, Some(12));
    assert_eq!(status_text, "12");
}

#[rstest]
fn sync_succeeds_on_zero_status(base_config: SyncConfig) {
    let cfg = base_config;
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let destination = SyncDestination::Local {
        path: Utf8PathBuf::from("/tmp/dst"),
    };
    assert!(syncer.sync(Utf8Path::new("/"), &destination).is_ok());
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
    let cfg = base_config;
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let _ = syncer
        .run_remote(&networking, "cargo test")
        .expect("run_remote should succeed");

    let args = syncer.build_remote_command("cargo test");
    assert!(
        args.starts_with("cd /remote/path && cargo test"),
        "remote command should change directory, got: {args}"
    );
}

#[test]
fn build_ssh_args_uses_wrapped_command_verbatim() {
    let cfg = base_config();
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let wrapped = syncer.build_remote_command("echo ok");
    let args = syncer.build_ssh_args(&networking(), &wrapped);

    assert_eq!(
        args.last(),
        Some(&OsString::from(wrapped.clone())),
        "ssh args should forward the already wrapped remote command"
    );
}
