//! Errors surfaced while performing a remote run.

use thiserror::Error;

use crate::sync::SyncError;

/// Errors surfaced while performing a remote run.
#[derive(Debug, Error)]
pub enum RunError<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// Raised when provisioning a new instance fails.
    #[error("failed to create instance: {0}")]
    Provision(#[source] Box<BackendError>),
    /// Raised when the instance does not become reachable over SSH.
    #[error("instance did not become ready: {message}")]
    Wait {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: Box<BackendError>,
    },
    /// Raised when cloud-init provisioning does not complete.
    #[error("instance provisioning did not complete: {message}")]
    Provisioning {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronization error.
        #[source]
        source: SyncError,
    },
    /// Raised when cloud-init provisioning does not complete before the timeout.
    #[error("instance provisioning did not complete: {message}")]
    ProvisioningTimeout {
        /// Human-readable description of the failure.
        message: String,
    },
    /// Raised when workspace synchronization fails.
    #[error("workspace sync failed: {message}")]
    Sync {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronization error.
        #[source]
        source: SyncError,
    },
    /// Raised when the remote command fails to start.
    #[error("remote command failed to start: {message}")]
    Remote {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying synchronization error.
        #[source]
        source: SyncError,
    },
    /// Raised when a configured cache cannot be mounted or verified.
    #[error("failed to mount the configured cache volume: {message}")]
    CacheMount {
        /// Human-readable description of the refusal.
        message: String,
        /// Underlying synchronization error, when the SSH call itself failed.
        #[source]
        source: Option<SyncError>,
    },
    /// Raised when teardown fails after the primary operation succeeded.
    #[error("failed to destroy instance: {message}")]
    Teardown {
        /// Human-readable description, including any unmount failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: Box<BackendError>,
    },
}
