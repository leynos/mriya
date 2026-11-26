//! Error types for the init workflow.

use camino::Utf8PathBuf;
use thiserror::Error;

use crate::{
    cancel::Signal,
    cleanup::CleanupFailure,
    config::ConfigError,
    config_store::ConfigStoreError,
    sync::SyncError,
};

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

/// Kind of failure that stopped initialization.
///
/// A caller uses this to classify the primary failure even when it is wrapped
/// in the volume context that `AfterVolume` adds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitErrorKind {
    /// Configuration update failed.
    Config,
    /// Volume creation failed.
    Volume,
    /// Helper-instance creation failed.
    Provision,
    /// Readiness polling failed.
    Wait,
    /// Formatting failed.
    Format,
    /// Ownership-aware cleanup failed.
    Cleanup,
    /// The ownership ledger could not record a change.
    Ledger,
    /// A signal interrupted the workflow.
    Cancelled,
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
    /// Raised when ownership-aware cleanup cannot verify its postconditions.
    #[error("cleanup failed: {0}")]
    Cleanup(CleanupFailure),
    /// Raised when the ownership ledger cannot record a change.
    #[error("ownership ledger error: {0}")]
    Ledger(String),
    /// Raised when initialization fails after the new volume was allocated.
    ///
    /// The primary error is preserved, and the note explains what happened to
    /// the volume: deleted before formatting, or retained because formatting
    /// completed and its identifier must reach configuration.
    #[error("{primary} ({note})")]
    AfterVolume {
        /// The primary failure, rendered.
        ///
        /// Holding the rendered message rather than the error itself avoids an
        /// infinitely recursive `Display` implementation, which the trait
        /// solver rejects.
        primary: String,
        /// Kind of the primary failure.
        kind: InitErrorKind,
        /// Identifier of the volume created for this run.
        volume_id: String,
        /// Zone that holds the volume.
        zone: String,
        /// What happened to the volume, including configuration instructions
        /// when it was retained.
        note: String,
        /// Typed cleanup failure, when cleanup could not verify its
        /// postconditions.
        cleanup: Option<Box<CleanupFailure>>,
    },
    /// Raised when a signal interrupted initialization.
    #[error("{message}")]
    Cancelled {
        /// Description naming the signal, the cleanup outcome and the journal.
        message: String,
        /// Signal that requested cancellation.
        signal: Signal,
        /// Typed cleanup failure, when cleanup could not verify its
        /// postconditions.
        cleanup: Option<Box<CleanupFailure>>,
        /// Journal that `mriya recover` can replay.
        journal: Option<Utf8PathBuf>,
    },
}

impl<BackendError> InitError<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// Returns the kind of failure this error reports.
    ///
    /// An `AfterVolume` wrapper reports the kind of the failure it annotates,
    /// so a caller classifies the primary cause rather than the wrapper.
    #[must_use]
    pub const fn kind(&self) -> InitErrorKind {
        match self {
            Self::Config(_) => InitErrorKind::Config,
            Self::Volume(_) => InitErrorKind::Volume,
            Self::Provision(_) => InitErrorKind::Provision,
            Self::Wait { .. } => InitErrorKind::Wait,
            Self::Format { .. } => InitErrorKind::Format,
            Self::Cleanup(_) => InitErrorKind::Cleanup,
            Self::Ledger(_) => InitErrorKind::Ledger,
            Self::AfterVolume { kind, .. } => *kind,
            Self::Cancelled { .. } => InitErrorKind::Cancelled,
        }
    }
}
