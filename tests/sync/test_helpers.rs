//! Test helpers for sync module behavioural tests.
//!
//! Provides workspace setup, temporary directory management, and fixture
//! utilities for testing the rsync synchronisation layer.

use cap_std::{ambient_authority, fs_utf8::Dir};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use mriya::InstanceNetworking;
use mriya::sync::{RemoteCommandOutput, SyncConfig, SyncError};
use rstest::fixture;
use tempfile::TempDir;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub local_root: Utf8PathBuf,
    pub remote_root: Utf8PathBuf,
    _local_tmp: Arc<TempDir>,
    _remote_tmp: Arc<TempDir>,
}

impl Workspace {
    pub fn new() -> Self {
        let local_tmp = Arc::new(temp_dir("local workspace"));
        let remote_tmp = Arc::new(temp_dir("remote workspace"));

        let local_root = Utf8PathBuf::from_path_buf(local_tmp.path().to_path_buf())
            .unwrap_or_else(|err| panic!("utf8 local path: {}", err.display()));
        let remote_root = Utf8PathBuf::from_path_buf(remote_tmp.path().to_path_buf())
            .unwrap_or_else(|err| panic!("utf8 remote path: {}", err.display()));

        Self {
            local_root,
            remote_root,
            _local_tmp: local_tmp,
            _remote_tmp: remote_tmp,
        }
    }
}

pub fn write_file(path: &Utf8Path, contents: &str) {
    let fs = Dir::open_ambient_dir("/", ambient_authority())
        .unwrap_or_else(|err| panic!("open ambient dir for writing: {err}"));
    let relative = path.strip_prefix("/").unwrap_or(path);
    if let Some(parent) = relative.parent() {
        fs.create_dir_all(parent)
            .unwrap_or_else(|err| panic!("create parent directories for {path}: {err}"));
    }
    fs.write(relative, contents)
        .unwrap_or_else(|err| panic!("write {path} content for test fixture: {err}"));
}

pub fn temp_dir(label: &str) -> TempDir {
    TempDir::new().unwrap_or_else(|err| panic!("{label}: {err}"))
}

pub fn utf8_path(path: std::path::PathBuf, label: &str) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(path).unwrap_or_else(|err| panic!("{label}: {}", err.display()))
}

#[derive(Clone, Debug)]
pub struct ScriptedContext {
    pub runner: super::test_doubles::ScriptedRunner,
    pub config: SyncConfig,
    pub networking: InstanceNetworking,
    pub source: Utf8PathBuf,
    pub _source_tmp: Arc<TempDir>,
}

pub fn build_scripted_context(
    runner: super::test_doubles::ScriptedRunner,
    label: &str,
) -> ScriptedContext {
    let source_tmp = Arc::new(temp_dir(label));
    let source_path = utf8_path(
        source_tmp.path().to_path_buf(),
        "scripted context source path",
    );

    ScriptedContext {
        runner,
        config: SyncConfig {
            rsync_bin: String::from("rsync"),
            ssh_bin: String::from("ssh"),
            ssh_user: String::from("ubuntu"),
            remote_path: String::from("/remote"),
            ssh_batch_mode: true,
            ssh_strict_host_key_checking: false,
            ssh_known_hosts_file: String::from("/dev/null"),
        },
        networking: InstanceNetworking {
            public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            ssh_port: 22,
        },
        source: source_path,
        _source_tmp: source_tmp,
    }
}

#[fixture]
pub fn workspace() -> Workspace {
    Workspace::new()
}

#[fixture]
pub fn scripted_context() -> ScriptedContext {
    build_scripted_context(
        super::test_doubles::ScriptedRunner::new(),
        "scripted context fixture",
    )
}

#[fixture]
pub fn output() -> RemoteCommandOutput {
    RemoteCommandOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }
}

#[fixture]
pub fn error() -> SyncError {
    SyncError::Spawn {
        program: String::from("rsync"),
        message: String::from("placeholder"),
    }
}
