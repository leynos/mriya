//! Shared fixtures for run BDD scenarios.

use camino::Utf8PathBuf;
use std::sync::Arc;
use mriya::sync::{SyncConfig, SyncError};
use mriya::{InstanceRequest, InstanceRequestBuilder};
use rstest::fixture;
use tempfile::TempDir;
use thiserror::Error;

use mriya::test_support::ScriptedRunner;
use super::test_doubles::{ScriptedBackend, ScriptedBackendError};

#[derive(Clone, Debug)]
pub struct RunContext {
    pub backend: ScriptedBackend,
    pub runner: ScriptedRunner,
    pub sync_config: SyncConfig,
    pub request: InstanceRequest,
    pub source: Utf8PathBuf,
}

#[derive(Clone, Debug)]
pub struct RunOutcome {
    pub backend: ScriptedBackend,
    pub result: Arc<
        Result<
            mriya::sync::RemoteCommandOutput,
            mriya::RunError<ScriptedBackendError, SyncError>,
        >,
    >,
}

#[derive(Debug, Error)]
pub enum RunTestError {
    #[error(transparent)]
    Sync(#[from] SyncError),
    #[error("failed to create workspace: {0}")]
    Workspace(String),
}

#[fixture]
pub fn run_context_result() -> Result<RunContext, RunTestError> {
    build_run_context()
}

#[fixture]
pub fn run_context(run_context_result: Result<RunContext, RunTestError>) -> RunContext {
    run_context_result.unwrap_or_else(|err| panic!("run context fixture should initialise: {err}"))
}

pub fn build_run_context() -> Result<RunContext, RunTestError> {
    let tmp_dir =
        TempDir::new().map_err(|err| RunTestError::Workspace(format!("tempdir: {err}")))?;
    let source = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).map_err(|path| {
        RunTestError::Workspace(format!("non-utf8 tempdir path: {}", path.display()))
    })?;

    Ok(RunContext {
        backend: ScriptedBackend::new(),
        runner: ScriptedRunner::new(),
        sync_config: sync_config(),
        request: request(),
        source,
    })
}

pub fn request() -> InstanceRequest {
    InstanceRequestBuilder::new()
        .image_label("ubuntu")
        .instance_type("DEV1-S")
        .zone("fr-par-1")
        .project_id("project")
        .architecture("x86_64")
        .build()
        .unwrap_or_else(|err| panic!("builder fixture should be valid: {err}"))
}

fn sync_config() -> SyncConfig {
    SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: String::from("/remote"),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
    }
}
