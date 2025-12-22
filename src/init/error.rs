//! Error types for the init workflow.

use thiserror::Error;

use crate::config::ConfigError;
use crate::config_store::ConfigStoreError;
use crate::sync::SyncError;

/// Errors raised while loading init configuration.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum InitConfigError {
    /// Raised when configuration parsing fails.
    #[error("init configuration parsing failed: {0}")]
    Parse(String),
    /// Raised when volume size is invalid.
    #[error("volume size must be greater than zero")]
    InvalidVolumeSize,
}

/// Errors raised while preparing an init request.
#[derive(Debug, Error)]
pub enum InitRequestError {
    /// Raised when Scaleway configuration is invalid.
    #[error("scaleway configuration error: {0}")]
    Config(#[from] ConfigError),
    /// Raised when init configuration is invalid.
    #[error("init configuration error: {0}")]
    InitConfig(#[from] InitConfigError),
    /// Raised when building the formatter instance request fails.
    #[error("instance request error: {0}")]
    RequestBuild(String),
    /// Raised when the computed volume name is invalid.
    #[error("volume name must not be empty")]
    InvalidVolumeName,
    /// Raised when the volume size cannot be represented in bytes.
    #[error("volume size is too large to represent")]
    SizeOverflow,
}

/// Errors raised while initialising a cache volume.
#[derive(Debug, Error)]
pub enum InitError<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// Raised when configuration updates fail.
    #[error("configuration update failed: {0}")]
    Config(#[from] ConfigStoreError),
    /// Raised when volume creation fails.
    #[error("failed to create volume: {0}")]
    Volume(#[source] BackendError),
    /// Raised when instance creation fails.
    #[error("failed to provision formatter instance: {0}")]
    Provision(#[source] BackendError),
    /// Raised when instance readiness checks fail.
    #[error("instance did not become ready: {message}")]
    Wait {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: BackendError,
    },
    /// Raised when formatting fails.
    #[error("volume format failed: {message}")]
    Format {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying sync error, if any.
        #[source]
        source: Option<SyncError>,
    },
    /// Raised when volume detachment fails.
    #[error("failed to detach volume: {message}")]
    Detach {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: BackendError,
    },
    /// Raised when teardown fails after formatting succeeds.
    #[error("failed to destroy formatter instance: {0}")]
    Teardown(#[source] BackendError),
}
