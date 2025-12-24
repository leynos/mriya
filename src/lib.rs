//! Core library for the Mriya remote execution tool.
//!
//! The crate exposes a backend abstraction for provisioning short‑lived
//! compute instances and a Scaleway implementation that powers the MVP
//! lifecycle (create → wait for SSH readiness → destroy).

pub mod backend;
pub mod cloud_init;
pub mod config;
pub mod config_store;
pub mod init;
pub mod janitor;
pub mod run;
pub mod scaleway;
pub mod sync;
#[cfg(test)]
pub mod test_helpers;
pub mod test_support;
pub mod volume;

pub use backend::{
    Backend, InstanceHandle, InstanceNetworking, InstanceRequest, InstanceRequestBuilder,
};
pub use config::ScalewayConfig;
pub use config_store::{ConfigStore, ConfigStoreError, ConfigWriter};
pub use init::{InitConfig, InitError, InitOrchestrator, InitOutcome, InitRequest};
pub use janitor::{
    Janitor, JanitorConfig, JanitorError, SweepSummary, TEST_RUN_ID_ENV, TEST_RUN_TAG_PREFIX,
};
pub use run::{RunError, RunOrchestrator};
pub use scaleway::{ScalewayBackend, ScalewayBackendError};
pub use sync::{
    CommandOutput, DEFAULT_REMOTE_PATH, ProcessCommandRunner, RemoteCommandOutput,
    StreamingCommandRunner, SyncConfig, SyncConfigLoadError, SyncDestination, SyncError, Syncer,
};
pub use volume::{VolumeBackend, VolumeHandle, VolumeRequest};
