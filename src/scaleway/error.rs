//! Error types for the Scaleway backend.

use scaleway_rs::ScalewayError;
use thiserror::Error;

use crate::{
    backend::BackendError,
    config::ConfigError,
    volume::{VolumeRequestError, VolumeTier},
};

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
    /// Raised when the instance never exposes a public IP.
    #[error("instance {instance_id} missing public IPv4 address")]
    MissingPublicIp {
        /// Provider instance identifier.
        instance_id: String,
    },
    /// Raised when teardown leaves a server visible in the API.
    #[error("instance {instance_id} still present after teardown")]
    ResidualResource {
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
    /// Wrapper for provider level failures.
    #[error("provider error: {message}")]
    Provider {
        /// Message returned by the provider SDK.
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
    /// Raised when a volume lookup finds nothing in the requested zone.
    #[error(
        "volume {volume_id} was not found in zone {zone}; the volume and the instance must be in the same Availability Zone"
    )]
    VolumeNotInZone {
        /// Volume identifier that was looked up.
        volume_id: String,
        /// Zone where lookup was attempted.
        zone: String,
    },
    /// Raised when a volume belongs to a different project than configured.
    #[error(
        "volume {volume_id} belongs to project {actual_project_id}, not the configured project {expected_project_id}; use a volume from the configured project"
    )]
    VolumeProjectMismatch {
        /// Volume identifier whose project did not match.
        volume_id: String,
        /// Project identifier from configuration.
        expected_project_id: String,
        /// Project identifier reported by the provider.
        actual_project_id: String,
    },
    /// Raised when a volume is already attached to a resource.
    #[error(
        "volume {volume_id} is already attached ({references}); detach it from `scw instance server detach-volume` or choose another volume"
    )]
    VolumeAlreadyAttached {
        /// Volume identifier that was already attached.
        volume_id: String,
        /// Comma-separated description of the current references.
        references: String,
    },
    /// Raised when a volume exists but is not in the `available` status.
    #[error(
        "volume {volume_id} is in status '{status}'; wait until it is 'available' before using it as a cache"
    )]
    VolumeUnavailable {
        /// Volume identifier that was not ready.
        volume_id: String,
        /// Status observed when the volume was rejected.
        status: String,
    },
    /// Raised when a volume never reaches the expected status.
    #[error(
        "timed out waiting for volume {volume_id} to become '{expected}'; last observed status was '{last_status}'"
    )]
    VolumeStatusTimeout {
        /// Volume identifier that was polled.
        volume_id: String,
        /// Status the caller was waiting for.
        expected: String,
        /// Last status observed before the deadline.
        last_status: String,
    },
    /// Raised when a configured volume uses the retired Block Storage class.
    #[error(
        "volume {volume_id} uses the legacy b_ssd storage class, which Mriya does not format, attach or reinterpret; migrate it with the snapshot export/import steps in docs/cache-volume-migration.md"
    )]
    VolumeLegacy {
        /// Legacy volume identifier.
        volume_id: String,
    },
    /// Raised when the requested tier is not offered in the zone.
    #[error(
        "volume tier '{tier}' is not available in zone {zone}; list the zone's volume types with `scw block volume-type list zone={zone}` or choose sbs_5k"
    )]
    VolumeTierUnavailable {
        /// Tier that was requested.
        tier: VolumeTier,
        /// Zone where the tier was requested.
        zone: String,
    },
    /// Raised when the provider rejects a volume deletion.
    #[error("failed to delete volume {volume_id}: {message}")]
    VolumeDeleteFailed {
        /// Volume identifier that could not be deleted.
        volume_id: String,
        /// Error message from the provider.
        message: String,
    },
    /// Raised when an accepted attachment returned an unverifiable map.
    #[error(
        "volume {volume_id} may be attached to instance {instance_id}, but the returned volume map could not be verified: {message}"
    )]
    VolumeAttachmentUnverified {
        /// Volume whose attachment could not be verified.
        volume_id: String,
        /// Instance that may hold the volume.
        instance_id: String,
        /// Description of the map violation.
        message: String,
    },
    /// Raised when a volume cannot be detached, so the server is retained.
    #[error(
        "failed to detach volume {volume_id} from instance {instance_id}: {message}; the server was retained to protect the cache. Recover with: scw instance server detach-volume server-id={instance_id} volume-id={volume_id} zone={zone}"
    )]
    VolumeDetachRetainsServer {
        /// Volume identifier that could not be detached.
        volume_id: String,
        /// Instance that still holds the volume.
        instance_id: String,
        /// Zone of both resources.
        zone: String,
        /// Error message from the provider.
        message: String,
    },
    /// Raised when rollback after a failed create could not clean up.
    #[error("{original} (rollback also failed: {cleanup})")]
    RollbackFailed {
        /// Original failure that triggered the rollback.
        original: Box<Self>,
        /// Description of the failed cleanup step.
        cleanup: String,
    },
}

impl From<VolumeRequestError> for ScalewayBackendError {
    fn from(value: VolumeRequestError) -> Self { Self::Validation(value.to_string()) }
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
            BackendError::VolumeIdNotUuid { value: id } => {
                Self::Validation(format!("volume_id '{id}' is not a canonical UUID"))
            }
        }
    }
}

impl From<ConfigError> for ScalewayBackendError {
    fn from(value: ConfigError) -> Self { Self::Config(value.to_string()) }
}
