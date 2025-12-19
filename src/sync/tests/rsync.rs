//! Tests for rsync argument construction and sync behaviour.

use super::super::*;
use crate::test_support::ScriptedRunner;
use rstest::rstest;
use tempfile::TempDir;

use super::fixtures::base_config;

#[rstest]
fn build_rsync_args_remote_includes_gitignore_filter(base_config: SyncConfig) {
    let runner = ScriptedRunner::new();
    let syncer = Syncer::new(base_config, runner).expect("config should validate");
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
    let runner = ScriptedRunner::new();
    let syncer = Syncer::new(base_config, runner).expect("config should validate");
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
    let runner = ScriptedRunner::new();
    runner.push_failure(12);
    let syncer = Syncer::new(base_config, runner).expect("config should validate");
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
    let runner = ScriptedRunner::new();
    runner.push_success();
    let syncer = Syncer::new(base_config, runner).expect("config should validate");
    let destination = SyncDestination::Local {
        path: Utf8PathBuf::from("/tmp/dst"),
    };
    assert!(syncer.sync(Utf8Path::new("/"), &destination).is_ok());
}
