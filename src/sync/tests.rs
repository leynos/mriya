//! Unit tests for the sync module.

use super::*;
use crate::backend::InstanceNetworking;
use crate::test_support::ScriptedRunner;
use rstest::{fixture, rstest};
use std::ffi::OsString;
use std::fmt::Write as _;
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

/// Helper to run a shell script via `StreamingCommandRunner` and assert expected output.
fn assert_streaming_runner_output(
    script: &str,
    expected_code: Option<i32>,
    expected_stdout: &str,
    expected_stderr: &str,
) {
    let runner = StreamingCommandRunner;
    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from(script)])
        .expect("command should execute successfully");

    assert_eq!(output.code, expected_code);
    assert_eq!(output.stdout, expected_stdout);
    assert_eq!(output.stderr, expected_stderr);
}

/// Helper to assert that SSH identity validation fails with the expected error.
fn assert_ssh_identity_validation_fails(
    base_config: SyncConfig,
    ssh_identity_file: Option<String>,
) {
    let cfg = SyncConfig {
        ssh_identity_file,
        ..base_config
    };
    let err = cfg
        .validate()
        .expect_err("ssh_identity_file validation should fail");
    let SyncError::InvalidConfig { ref field } = err else {
        panic!("expected InvalidConfig, got {err:?}");
    };
    assert_eq!(field, "ssh_identity_file");
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
        ssh_identity_file: Some(String::from("~/.ssh/id_ed25519")),
        volume_mount_path: String::from("/mriya"),
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

#[rstest]
fn streaming_runner_captures_output() {
    assert_streaming_runner_output("printf out && printf err 1>&2", Some(0), "out", "err");
}

#[rstest]
fn streaming_runner_captures_output_on_failure() {
    assert_streaming_runner_output(
        "printf out && printf err 1>&2; exit 42",
        Some(42),
        "out",
        "err",
    );
}

#[rstest]
fn streaming_runner_propagates_non_zero_exit_code() {
    let runner = StreamingCommandRunner;
    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from("exit 7")])
        .expect("command should execute successfully");

    assert_eq!(output.code, Some(7));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[rstest]
fn streaming_runner_handles_no_output() {
    let runner = StreamingCommandRunner;
    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from("")])
        .expect("command should execute successfully");

    assert_eq!(output.code, Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[rstest]
fn streaming_runner_captures_large_interleaved_output() {
    let runner = StreamingCommandRunner;
    let script = r#"
        for i in $(seq 1 50); do
            printf "out-%03d\n" "$i"
            printf "err-%03d\n" "$i" 1>&2
        done
    "#;

    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from(script)])
        .expect("command should execute successfully");

    let mut expected_out = String::new();
    let mut expected_err = String::new();
    for i in 1..=50 {
        writeln!(&mut expected_out, "out-{i:03}").expect("write expected_out");
        writeln!(&mut expected_err, "err-{i:03}").expect("write expected_err");
    }

    assert_eq!(output.code, Some(0));
    assert_eq!(output.stdout, expected_out);
    assert_eq!(output.stderr, expected_err);
}

#[rstest]
fn streaming_runner_failed_spawn_returns_spawn_error() {
    let runner = StreamingCommandRunner;
    let result = runner.run("definitely-not-a-real-binary-xyz", &[]);

    match result {
        Err(SyncError::Spawn { .. }) => {}
        other => panic!("expected SyncError::Spawn, got {other:?}"),
    }
}

#[rstest]
fn sync_config_validation_rejects_missing_ssh_identity(base_config: SyncConfig) {
    assert_ssh_identity_validation_fails(base_config, None);
}

#[rstest]
fn sync_config_validation_rejects_empty_ssh_identity(base_config: SyncConfig) {
    assert_ssh_identity_validation_fails(base_config, Some(String::from("  ")));
}

#[rstest]
fn sync_error_invalid_config_produces_actionable_message(base_config: SyncConfig) {
    let cfg = SyncConfig {
        ssh_identity_file: None,
        ..base_config
    };
    let err = cfg
        .validate()
        .expect_err("missing ssh_identity_file should fail");
    let message = err.to_string();
    assert!(
        message.contains("MRIYA_SYNC_SSH_IDENTITY_FILE"),
        "error should mention env var: {message}"
    );
    assert!(
        message.contains("mriya.toml"),
        "error should mention config file: {message}"
    );
}

#[rstest]
fn common_ssh_options_includes_identity_flag(
    base_config: SyncConfig,
    networking: InstanceNetworking,
) {
    let cfg = SyncConfig {
        ssh_identity_file: Some(String::from("/path/to/key")),
        ..base_config
    };
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(cfg, runner).expect("config should validate");
    let args = syncer.build_ssh_args(&networking, "echo ok");
    let args_strs: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    assert!(
        args_strs.contains(&String::from("-i")),
        "should include -i flag: {args_strs:?}"
    );
    assert!(
        args_strs.contains(&String::from("/path/to/key")),
        "should include key path: {args_strs:?}"
    );
}

#[rstest]
fn rsync_remote_shell_includes_identity_flag(base_config: SyncConfig) {
    let cfg = SyncConfig {
        ssh_identity_file: Some(String::from("/path/to/key")),
        ..base_config
    };
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

    let rsh_arg = args_strs
        .iter()
        .find(|arg| arg.contains("ssh") && arg.contains("-i"))
        .expect("rsync --rsh should include -i flag");
    assert!(
        rsh_arg.contains("/path/to/key"),
        "remote shell should include key path: {rsh_arg}"
    );
}

#[test]
fn expand_tilde_expands_home_prefix() {
    let home = std::env::var("HOME").expect("HOME should be set");
    let expanded = expand_tilde("~/.ssh/id_ed25519");
    assert_eq!(expanded, format!("{home}/.ssh/id_ed25519"));
}

#[test]
fn expand_tilde_leaves_absolute_paths_unchanged() {
    let path = "/absolute/path/to/key";
    assert_eq!(expand_tilde(path), path);
}

#[test]
fn expand_tilde_leaves_relative_paths_unchanged() {
    let path = "relative/path/to/key";
    assert_eq!(expand_tilde(path), path);
}
