//! Error types for the init workflow.

use thiserror::Error;

use crate::{
    config::ConfigError,
    config_store::ConfigStoreError,
    sync::SyncError,
    volume::VolumeTierParseError,
};

/// Errors raised while loading init configuration.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum InitConfigError {
    /// Raised when configuration parsing fails.
    #[error("init configuration parsing failed: {0}")]
    Parse(String),
    /// Raised when the configured volume size is below the provider minimum.
    #[error(
        "volume size must be at least 5 decimal GB (5,000,000,000 bytes); set MRIYA_INIT_VOLUME_SIZE_GB to a larger value"
    )]
    InvalidVolumeSize,
    /// Raised when the performance tier is not recognized.
    #[error("unsupported volume tier '{value}'; set MRIYA_INIT_VOLUME_TIER to sbs_5k or sbs_15k")]
    InvalidVolumeTier {
        /// Tier value that was rejected.
        value: String,
    },
}

impl From<VolumeTierParseError> for InitConfigError {
    fn from(value: VolumeTierParseError) -> Self { Self::InvalidVolumeTier { value: value.value } }
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

/// Errors raised while initializing a cache volume.
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
    Volume(#[source] Box<BackendError>),
    /// Raised when the new volume never becomes available.
    #[error("new volume did not become available: {message}")]
    VolumeReady {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: Box<BackendError>,
    },
    /// Raised when instance creation fails.
    #[error("failed to provision formatter instance: {0}")]
    Provision(#[source] Box<BackendError>),
    /// Raised when instance readiness checks fail.
    #[error("instance did not become ready: {message}")]
    Wait {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: Box<BackendError>,
    },
    /// Raised when no guest disk carries the new volume's serial.
    #[error("cannot format volume {volume_id}: no disk reports its serial ({message})")]
    FormatDeviceNotFound {
        /// Volume the formatter was looking for.
        volume_id: String,
        /// Parsed status details.
        message: String,
    },
    /// Raised when more than one guest disk carries the volume's serial.
    #[error("cannot format volume {volume_id}: more than one disk matches ({message})")]
    FormatDeviceAmbiguous {
        /// Volume the formatter was looking for.
        volume_id: String,
        /// Parsed status details.
        message: String,
    },
    /// Raised when the resolved device is the root disk or its ancestor.
    #[error(
        "refusing to format volume {volume_id}: the resolved device is the root disk or one of its ancestors ({message})"
    )]
    FormatRootDisk {
        /// Volume whose device resolved to the root disk.
        volume_id: String,
        /// Parsed status details.
        message: String,
    },
    /// Raised when the guest's root device cannot be resolved.
    #[error(
        "refusing to format volume {volume_id}: the guest root device could not be identified safely ({message})"
    )]
    FormatRootUnknown {
        /// Volume whose safety could not be established.
        volume_id: String,
        /// Parsed status details.
        message: String,
    },
    /// Raised when the device already carries a filesystem signature.
    #[error(
        "refusing to format volume {volume_id}: the device already has a filesystem signature ({message})"
    )]
    FormatSignaturePresent {
        /// Volume that was not formatted.
        volume_id: String,
        /// Parsed status details, including the detected filesystem type.
        message: String,
    },
    /// Raised when formatting fails for any other reason.
    #[error("volume format failed for {volume_id}: {message}")]
    FormatFailed {
        /// Volume that could not be formatted.
        volume_id: String,
        /// Human-readable description of the failure.
        message: String,
        /// Underlying sync error, if any.
        #[source]
        source: Option<Box<SyncError>>,
    },
    /// Raised when volume detachment fails.
    #[error("failed to detach volume: {message}")]
    Detach {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: Box<BackendError>,
    },
    /// Raised when the formatter cannot be stopped or detached.
    #[error("failed to tear down formatter instance: {message}")]
    FormatterTeardown {
        /// Failure description naming the formatter, cache volume and recovery
        /// steps.
        message: String,
        /// Provider-specific error.
        #[source]
        source: Box<BackendError>,
    },
    /// Raised when the formatter server cannot be destroyed after detachment.
    #[error(
        "failed to destroy formatter instance: {message}; volume {volume_id} was detached in zone {zone}; delete it with `scw block volume delete {volume_id} zone={zone}`"
    )]
    TeardownAfterDetach {
        /// New volume that was detached but not recorded.
        volume_id: String,
        /// Zone that holds the volume.
        zone: String,
        /// Human-readable description of the failure, including the formatter
        /// removal command.
        message: String,
        /// Provider-specific error.
        #[source]
        source: Box<BackendError>,
    },
    /// Raised when cleanup after a failed init could not remove everything.
    #[error("{original} (rollback also failed: {cleanup})")]
    Rollback {
        /// Original failure that triggered the cleanup, kept as a trait object
        /// so the display text survives without making the enum recursive.
        #[source]
        original: Box<dyn std::error::Error + Send + Sync + 'static>,
        /// Description of the failed cleanup steps, including any retained
        /// volume identifier for manual recovery.
        cleanup: String,
    },
}
