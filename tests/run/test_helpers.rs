//! Shared fixtures for run BDD scenarios.

use std::time::Duration;

use camino::Utf8PathBuf;
use mriya::sync::{RemoteCommandOutput, SyncConfig, SyncError};
use mriya::{InstanceRequest, InstanceRequestBuilder};
use rstest::fixture;
use tempfile::TempDir;
use thiserror::Error;

use super::test_doubles::ScriptedBackend;
use crate::sync_config::sync_config;
use crate::test_constants::DEFAULT_INSTANCE_TYPE;
use mriya::test_support::ScriptedRunner;

#[derive(Clone, Debug)]
pub struct RunContext {
    pub backend: ScriptedBackend,
    pub runner: ScriptedRunner,
    pub sync_config: SyncConfig,
    pub request: InstanceRequest,
    pub source: Utf8PathBuf,
    pub cloud_init_poll_interval_override: Option<Duration>,
    pub cloud_init_wait_timeout_override: Option<Duration>,
    pub outcome: Option<RunResult>,
    pub(crate) source_tmp: std::sync::Arc<TempDir>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunFailureKind {
    Provision,
    Wait,
    Provisioning,
    ProvisioningTimeout,
    Sync,
    Remote,
    Teardown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunFailure {
    pub kind: RunFailureKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum RunResult {
    Success(RemoteCommandOutput),
    Failure(RunFailure),
}

#[derive(Clone, Debug, Error)]
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
        cloud_init_poll_interval_override: None,
        cloud_init_wait_timeout_override: None,
        outcome: None,
        source_tmp: std::sync::Arc::new(tmp_dir),
    })
}

pub fn request() -> InstanceRequest {
    InstanceRequestBuilder::new()
        .image_label("ubuntu")
        .instance_type(DEFAULT_INSTANCE_TYPE)
        .zone("fr-par-1")
        .project_id("project")
        .architecture("x86_64")
        .build()
        .unwrap_or_else(|err| panic!("builder fixture should be valid: {err}"))
}
