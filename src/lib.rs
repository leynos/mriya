//! Core library for the Mriya remote execution tool.
//!
//! The crate exposes a backend abstraction for provisioning short‑lived
//! compute instances and a Scaleway implementation that powers the MVP
//! lifecycle (create → wait for SSH readiness → destroy).

pub mod backend;
pub mod config;
pub mod scaleway;
pub mod sync;
pub mod test_support;

pub use backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest};
pub use config::ScalewayConfig;
pub use scaleway::{ScalewayBackend, ScalewayBackendError};
pub use sync::{
    CommandOutput, DEFAULT_REMOTE_PATH, ProcessCommandRunner, RemoteCommandOutput, SyncConfig,
    SyncDestination, SyncError, Syncer,
};
