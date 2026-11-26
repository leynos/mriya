//! Error types for the Scaleway backend.

use scaleway_rs::ScalewayError;
use thiserror::Error;

use crate::{
    backend::BackendError,
    boot_volume::{BootVolumeMismatch, BootVolumeType},
    config::ConfigError,
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
    /// Raised when an image exists but its root volume cannot back the
    /// requested boot-storage type.
    #[error(
        "image '{label}' (arch {arch}) in zone {zone} has root volume type \
         '{observed}', which cannot back {requested}",
        requested = requested.map_or_else(|| String::from("any boot volume"), |kind| format!("a {kind} boot volume"))
    )]
    ImageStorageIncompatible {
        /// Image label passed by the caller.
        label: String,
        /// Architecture requested by the caller.
        arch: String,
        /// Zone used for the lookup.
        zone: String,
        /// Distinct root-volume types the candidate images reported.
        observed: String,
        /// Storage type the profile asked for, if any.
        requested: Option<BootVolumeType>,
    },
    /// Raised when the zone's product catalogue does not list the instance
    /// type, so its storage constraints cannot be checked.
    #[error("instance type '{instance_type}' is not listed in zone {zone}'s product catalogue")]
    ProductCatalogueUnavailable {
        /// Requested commercial type.
        instance_type: String,
        /// Zone whose catalogue was read.
        zone: String,
    },
    /// Raised when the instance type offers a different architecture.
    #[error(
        "instance type '{instance_type}' offers architecture '{offered_arch}', \
         but the request asks for '{requested_arch}'"
    )]
    BootVolumeArchitectureMismatch {
        /// Requested commercial type.
        instance_type: String,
        /// Architecture the request asks for.
        requested_arch: String,
        /// Architecture the catalogue reports for the type.
        offered_arch: String,
    },
    /// Raised when the requested storage type is not available for the
    /// instance type or the image.
    #[error(
        "cannot use a {volume_type} boot volume on instance type \
         '{instance_type}': {reason}"
    )]
    BootVolumeTypeUnavailable {
        /// Requested commercial type.
        instance_type: String,
        /// Storage type that cannot be used.
        volume_type: BootVolumeType,
        /// Why the combination is refused.
        reason: String,
    },
    /// Raised when the requested size cannot hold the image.
    #[error(
        "boot volume of {size_bytes} bytes is smaller than the image's \
         {image_size_bytes} byte root volume, which a {volume_type} boot disk \
         on instance type '{instance_type}' must cover"
    )]
    BootVolumeBelowImageSize {
        /// Requested commercial type.
        instance_type: String,
        /// Storage type being planned.
        volume_type: BootVolumeType,
        /// Requested size in bytes.
        size_bytes: u64,
        /// Image root size in bytes.
        image_size_bytes: u64,
    },
    /// Raised when the requested size falls outside the accepted range.
    #[error(
        "boot volume of {size_bytes} bytes is outside the {min_size}..={max_size} \
         byte range instance type '{instance_type}' accepts for {volume_type}"
    )]
    BootVolumeSizeOutOfBounds {
        /// Requested commercial type.
        instance_type: String,
        /// Storage type being planned.
        volume_type: BootVolumeType,
        /// Requested size in bytes.
        size_bytes: u64,
        /// Smallest accepted size in bytes.
        min_size: u64,
        /// Largest accepted size in bytes.
        max_size: u64,
    },
    /// Raised when a computed size is not a multiple of the provider's
    /// 512-byte increment.
    #[error("boot volume size {size_bytes} is not a multiple of {alignment} bytes")]
    BootVolumeSizeUnaligned {
        /// Size that failed alignment.
        size_bytes: u64,
        /// Required increment in bytes.
        alignment: u64,
    },
    /// Raised when converting a configured size fails.
    #[error("invalid boot volume setting: {message}")]
    BootVolumeInvalid {
        /// Description of the conversion failure.
        message: String,
    },
    /// Raised when the root volume the provider reported differs from the plan.
    #[error(transparent)]
    BootVolumeMismatch(#[from] BootVolumeMismatch),
    /// Raised when reading an attached Block Storage volume fails.
    #[error("failed to read Block Storage volume {volume_id} in zone {zone}: {message}")]
    BlockVolumeReadFailed {
        /// Volume identifier that could not be read.
        volume_id: String,
        /// Zone where the read was attempted.
        zone: String,
        /// Error message from the provider.
        message: String,
    },
    /// Raised when a post-create step failed and the best-effort teardown also
    /// failed, so both outcomes are reported rather than the second masking
    /// the first.
    #[error("{message}")]
    CreateCleanupFailed {
        /// The original failure, with the teardown note appended.
        message: String,
        /// The original failure, kept so callers can still classify it.
        #[source]
        source: Box<ScalewayBackendError>,
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
