//! Tests for SSH option construction and identity forwarding.

use super::super::*;
use crate::backend::InstanceNetworking;
use crate::test_support::ScriptedRunner;
use rstest::rstest;
use tempfile::TempDir;

use super::fixtures::{base_config, networking};

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
