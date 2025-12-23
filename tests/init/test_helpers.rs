//! Shared fixtures for init BDD scenarios.

use mriya::sync::{SyncConfig, SyncError};
use mriya::test_support::ScriptedRunner;
use mriya::{InitRequest, InstanceRequestBuilder, VolumeRequest};
use rstest::fixture;
use thiserror::Error;

use super::test_doubles::{MemoryConfigStore, ScriptedVolumeBackend};
use crate::size_constants::BYTES_PER_GB;
use crate::sync_config::sync_config;
use crate::test_constants::DEFAULT_INSTANCE_TYPE;

#[derive(Clone, Debug)]
pub struct InitContext {
    pub backend: ScriptedVolumeBackend,
    pub runner: ScriptedRunner,
    pub sync_config: SyncConfig,
    pub request: InitRequest,
    pub config_store: MemoryConfigStore,
    pub outcome: Option<InitResult>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InitFailureKind {
    Volume,
    Provision,
    Wait,
    Format,
    Detach,
    Teardown,
    Config,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitFailure {
    pub kind: InitFailureKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum InitResult {
    Success,
    Failure(InitFailure),
}

#[derive(Clone, Debug, Error)]
pub enum InitTestError {
    #[error(transparent)]
    Sync(#[from] SyncError),
    #[error("invalid init fixture: {0}")]
    Fixture(String),
}

#[fixture]
pub fn init_context_result() -> Result<InitContext, InitTestError> {
    build_init_context()
}

#[fixture]
pub fn init_context(init_context_result: Result<InitContext, InitTestError>) -> InitContext {
    init_context_result
        .unwrap_or_else(|err| panic!("init context fixture should initialise: {err}"))
}

fn build_init_context() -> Result<InitContext, InitTestError> {
    let sync_config = sync_config();

    let instance_request = InstanceRequestBuilder::new()
        .image_label("ubuntu")
        .instance_type(DEFAULT_INSTANCE_TYPE)
        .zone("fr-par-1")
        .project_id("project")
        .architecture("x86_64")
        .build()
        .map_err(|err| InitTestError::Fixture(format!("instance request: {err}")))?;

    let volume = VolumeRequest::new("mriya-test-cache", 10 * BYTES_PER_GB, "fr-par-1", "project");

    Ok(InitContext {
        backend: ScriptedVolumeBackend::new(),
        runner: ScriptedRunner::new(),
        sync_config,
        request: InitRequest {
            volume,
            instance_request,
            overwrite_existing_volume_id: false,
        },
        config_store: MemoryConfigStore::new(),
        outcome: None,
    })
}
