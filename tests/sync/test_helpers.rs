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
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub local_root: Utf8PathBuf,
    pub remote_root: Utf8PathBuf,
    _local_tmp: Arc<TempDir>,
    _remote_tmp: Arc<TempDir>,
}

impl Default for Workspace {
    fn default() -> Self {
        match Self::new() {
            Ok(ws) => ws,
            Err(err) => panic!("workspace default should succeed: {err}"),
        }
    }
}

impl Workspace {
    pub fn new() -> Result<Self, SyncError> {
        let local_tmp = Arc::new(temp_dir("local workspace")?);
        let remote_tmp = Arc::new(temp_dir("remote workspace")?);

        let local_root =
            Utf8PathBuf::from_path_buf(local_tmp.path().to_path_buf()).map_err(|err| {
                SyncError::Spawn {
                    program: String::from("fixture"),
                    message: err.display().to_string(),
                }
            })?;
        let remote_root =
            Utf8PathBuf::from_path_buf(remote_tmp.path().to_path_buf()).map_err(|err| {
                SyncError::Spawn {
                    program: String::from("fixture"),
                    message: err.display().to_string(),
                }
            })?;

        Ok(Self {
            local_root,
            remote_root,
            _local_tmp: local_tmp,
            _remote_tmp: remote_tmp,
        })
    }
}

pub fn write_file(path: &Utf8Path, contents: &str) -> Result<(), SyncError> {
    let fs = Dir::open_ambient_dir("/", ambient_authority()).map_err(|err| SyncError::Spawn {
        program: String::from("fixture"),
        message: err.to_string(),
    })?;
    let relative = path.strip_prefix("/").unwrap_or(path);
    if let Some(parent) = relative.parent() {
        fs.create_dir_all(parent).map_err(|err| SyncError::Spawn {
            program: String::from("fixture"),
            message: err.to_string(),
        })?;
    }
    fs.write(relative, contents)
        .map_err(|err| SyncError::Spawn {
            program: String::from("fixture"),
            message: err.to_string(),
        })
}

pub fn temp_dir(label: &str) -> Result<TempDir, SyncError> {
    TempDir::new().map_err(|err| SyncError::Spawn {
        program: String::from(label),
        message: err.to_string(),
    })
}

pub fn utf8_path(path: std::path::PathBuf, label: &str) -> Result<Utf8PathBuf, SyncError> {
    Utf8PathBuf::from_path_buf(path).map_err(|err| SyncError::Spawn {
        program: String::from(label),
        message: err.display().to_string(),
    })
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
) -> Result<ScriptedContext, SyncError> {
    let source_tmp = Arc::new(temp_dir(label)?);
    let source_path = utf8_path(
        source_tmp.path().to_path_buf(),
        "scripted context source path",
    )?;

    Ok(ScriptedContext {
        runner,
        config: SyncConfig {
            rsync_bin: String::from("rsync"),
            ssh_bin: String::from("ssh"),
            ssh_user: String::from("ubuntu"),
            remote_path: String::from("/remote"),
            ssh_batch_mode: true,
            ssh_strict_host_key_checking: false,
            ssh_known_hosts_file: String::from("/dev/null"),
            ssh_identity_file: Some(String::from("~/.ssh/id_ed25519")),
            volume_mount_path: String::from("/mriya"),
            route_build_caches: true,
        },
        networking: InstanceNetworking {
            public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            ssh_port: 22,
        },
        source: source_path,
        _source_tmp: source_tmp,
    })
}

#[fixture]
pub fn workspace_result() -> Result<Workspace, SyncError> {
    Workspace::new()
}

#[fixture]
pub fn workspace(workspace_result: Result<Workspace, SyncError>) -> Workspace {
    workspace_result.unwrap_or_else(|err| panic!("workspace fixture should initialise: {err}"))
}

#[fixture]
pub fn scripted_context_result() -> Result<ScriptedContext, SyncError> {
    let ctx = build_scripted_context(
        super::test_doubles::ScriptedRunner::new(),
        "scripted context fixture",
    )?;
    Ok(ctx)
}

#[fixture]
pub fn scripted_context(
    scripted_context_result: Result<ScriptedContext, SyncError>,
) -> ScriptedContext {
    scripted_context_result
        .unwrap_or_else(|err| panic!("scripted context fixture should initialise: {err}"))
}

#[fixture]
pub fn output() -> RemoteCommandOutput {
    RemoteCommandOutput {
        exit_code: Some(0),
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

#[fixture]
pub fn networking() -> InstanceNetworking {
    InstanceNetworking {
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        ssh_port: 22,
    }
}

#[fixture]
pub fn base_sync_config() -> SyncConfig {
    SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: String::from("/remote"),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
        ssh_identity_file: Some(String::from("~/.ssh/id_ed25519")),
        volume_mount_path: String::from("/mriya"),
        route_build_caches: true,
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum StepError {
    #[error(transparent)]
    Sync(#[from] SyncError),
    #[error("assertion failed: {0}")]
    Assertion(String),
}
