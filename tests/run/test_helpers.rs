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
    /// The command the scenario asked the orchestrator to run remotely.
    ///
    /// Steps assert against this instead of matching a literal, so a scenario
    /// that changes its command cannot keep passing on a stale text search.
    pub requested_command: Option<String>,
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

pub type RunContextResult = Result<RunContext, RunTestError>;

#[mriya_test_macros::allow_fixture_expansion_lints]
#[fixture]
pub fn run_context_result() -> RunContextResult {
    build_run_context()
}

pub fn build_run_context() -> RunContextResult {
    let tmp_dir =
        TempDir::new().map_err(|err| RunTestError::Workspace(format!("tempdir: {err}")))?;
    let source = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).map_err(|path| {
        RunTestError::Workspace(format!("non-utf8 tempdir path: {}", path.display()))
    })?;

    Ok(RunContext {
        backend: ScriptedBackend::new(),
        runner: ScriptedRunner::new(),
        sync_config: sync_config(),
        request: request()?,
        source,
        cloud_init_poll_interval_override: None,
        cloud_init_wait_timeout_override: None,
        outcome: None,
        requested_command: None,
        source_tmp: std::sync::Arc::new(tmp_dir),
    })
}

pub fn request() -> Result<InstanceRequest, RunTestError> {
    InstanceRequestBuilder::new()
        .image_label("ubuntu")
        .instance_type(DEFAULT_INSTANCE_TYPE)
        .zone("fr-par-1")
        .project_id("project")
        .architecture("x86_64")
        .build()
        .map_err(|err| RunTestError::Workspace(format!("instance request: {err}")))
}
