//! Error types for the Scaleway backend.

use scaleway_rs::ScalewayError;
use thiserror::Error;

use crate::{backend::BackendError, config::ConfigError, ownership::ResourceKind};

/// Errors raised by the Scaleway backend.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScalewayBackendError {
    /// Raised when the high-level configuration is incomplete.
    #[error("configuration error: {0}")]
    Config(String),
    /// Raised when a request is missing a required field.
    #[error("invalid instance request: {0}")]
    Validation(String),
    /// Raised when the requested image label cannot be resolved.
    #[error("image '{label}' (arch {arch}) not found in zone {zone}")]
    ImageNotFound {
        /// Image label passed by the caller.
        label: String,
        /// Architecture requested by the caller.
        arch: String,
        /// Zone used for the lookup.
        zone: String,
    },
    /// Raised when the server type is not available in the selected zone.
    #[error("instance type '{instance_type}' not available in zone {zone}")]
    InstanceTypeUnavailable {
        /// Requested commercial type.
        instance_type: String,
        /// Target zone.
        zone: String,
    },
    /// Raised when an asynchronous operation exceeds the timeout.
    #[error("timeout waiting for {action} on instance {instance_id}")]
    Timeout {
        /// Action being waited on.
        action: String,
        /// Provider instance identifier.
        instance_id: String,
    },
    /// Raised when waiting for a volume state exceeds the timeout.
    #[error("timeout waiting for {action} on volume {volume_id}")]
    VolumeTimeout {
        /// Action being waited on.
        action: String,
        /// Provider volume identifier.
        volume_id: String,
    },
    /// Raised when the instance never exposes a public IP.
    #[error("instance {instance_id} missing public IPv4 address")]
    MissingPublicIp {
        /// Provider instance identifier.
        instance_id: String,
    },
    /// Raised when an instance cannot be powered on.
    #[error("instance {instance_id} in state {state} cannot be powered on")]
    PowerOnNotAllowed {
        /// Provider instance identifier.
        instance_id: String,
        /// Current state reported by the provider.
        state: String,
    },
    /// Raised when the provider answered 404 for an operation.
    ///
    /// A 404 is evidence, not proof: absence is confirmed only by a
    /// successful, identity-checked listing.
    #[error("{operation} returned not found: {message}")]
    NotFound {
        /// Operation that returned 404.
        operation: String,
        /// Provider message for the response.
        message: String,
    },
    /// Raised when the provider denied authentication or authorization.
    ///
    /// Cleanup treats this as a failure: it cannot prove the resource is
    /// gone, and silently ignoring it would report a leak as clean.
    #[error("{operation} was denied with HTTP {status}: {message}")]
    AccessDenied {
        /// Operation that was denied.
        operation: String,
        /// HTTP status, 401 or 403.
        status: u16,
        /// Provider message for the response.
        message: String,
    },
    /// Raised when a create request may or may not have been applied.
    #[error("{operation} outcome is ambiguous: {message}")]
    AmbiguousOutcome {
        /// Operation whose outcome is unknown.
        operation: String,
        /// Why the outcome could not be decided.
        message: String,
    },
    /// Raised when reconciliation found no resource carrying the allocation
    /// tag after an ambiguous create.
    #[error(
        "allocation outcome unknown for {kind} {tag} in zone {zone}: {detail}; \
         run `mriya recover` to reconcile"
    )]
    AllocationOutcomeUnknown {
        /// Kind of resource whose allocation is uncertain.
        kind: ResourceKind,
        /// Allocation correlation tag.
        tag: String,
        /// Zone the allocation was requested in.
        zone: String,
        /// Why the outcome could not be decided.
        detail: String,
    },
    /// Wrapper for provider level failures.
    #[error("provider error: {message}")]
    Provider {
        /// Message returned by the provider SDK.
        message: String,
    },
    /// Raised when the ownership journal cannot be written.
    #[error("failed to persist the ownership journal: {message}")]
    Journal {
        /// Human-readable persistence failure.
        message: String,
    },
    /// Raised when a volume cannot be attached to an instance.
    #[error("failed to attach volume {volume_id} to instance {instance_id}: {message}")]
    VolumeAttachmentFailed {
        /// Volume identifier that could not be attached.
        volume_id: String,
        /// Instance identifier.
        instance_id: String,
        /// Error message from the provider.
        message: String,
    },
    /// Raised when a volume cannot be detached from an instance.
    #[error("failed to detach volume {volume_id} from instance {instance_id}: {message}")]
    VolumeDetachFailed {
        /// Volume identifier that could not be detached.
        volume_id: String,
        /// Instance identifier.
        instance_id: String,
        /// Error message from the provider.
        message: String,
    },
    /// Raised when a volume cannot be created.
    #[error("failed to create volume {name} in zone {zone}: {message}")]
    VolumeCreateFailed {
        /// Volume name requested.
        name: String,
        /// Zone where creation was attempted.
        zone: String,
        /// Error message from the provider.
        message: String,
    },
    /// Raised when the specified volume does not exist or is not accessible.
    #[error("volume {volume_id} not found in zone {zone}")]
    VolumeNotFound {
        /// Volume identifier that was not found.
        volume_id: String,
        /// Zone where lookup was attempted.
        zone: String,
    },
}

impl From<ScalewayError> for ScalewayBackendError {
    fn from(value: ScalewayError) -> Self {
        Self::Provider {
            message: value.to_string(),
        }
    }
}

impl From<BackendError> for ScalewayBackendError {
    fn from(value: BackendError) -> Self {
        match value {
            BackendError::Validation(field) => Self::Validation(field),
        }
    }
}

impl From<ConfigError> for ScalewayBackendError {
    fn from(value: ConfigError) -> Self { Self::Config(value.to_string()) }
}
