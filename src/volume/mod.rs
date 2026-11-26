//! Volume lifecycle abstractions for persistent cache storage.
//!
//! Scaleway sizes Block Storage volumes in decimal gigabytes (10^9 bytes) and
//! offers two performance tiers. The types here keep those facts in the port so
//! orchestrators do not have to know the provider's JSON shape. Creation,
//! lookup, readiness, attachment, detachment and deletion are all expressed as
//! backend operations so tests can substitute a scripted implementation.

use std::fmt;

use crate::backend::{Backend, BackendFuture, InstanceHandle};

/// Bytes in one decimal gigabyte (10^9), the granularity Scaleway uses for
/// Block Storage sizes.
pub const BYTES_PER_GB: u64 = 1_000_000_000;
/// Smallest Block Storage volume Scaleway accepts, in decimal gigabytes.
pub const MIN_VOLUME_SIZE_GB: u64 = 5;
/// Largest Block Storage volume Scaleway documents, in decimal gigabytes.
pub const MAX_VOLUME_SIZE_GB: u64 = 15_000;

mod state;
mod tier;

pub use state::{VolumeReference, VolumeState, VolumeStatus};
pub use tier::{VolumeTier, VolumeTierParseError};

/// Parameters required to create a persistent volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeRequest {
    /// Human-friendly volume name.
    pub name: String,
    /// Size in decimal bytes (1 GB is 1,000,000,000 bytes).
    pub size_bytes: u64,
    /// Target availability zone.
    pub zone: String,
    /// Project identifier used for billing and ownership.
    pub project_id: String,
    /// Optional organization identifier when the provider requires one.
    pub organisation_id: Option<String>,
    /// Performance tier requested for the volume.
    pub tier: VolumeTier,
}

impl VolumeRequest {
    /// Creates a new volume request, trimming string fields.
    ///
    /// The tier defaults to [`VolumeTier::Sbs5k`]; use
    /// [`VolumeRequest::with_tier`] to request another.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::volume::{VolumeRequest, VolumeTier};
    ///
    /// let request = VolumeRequest::new("cache", 20_000_000_000, "fr-par-1", "project");
    /// assert_eq!(request.tier, VolumeTier::Sbs5k);
    /// ```
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        size_bytes: u64,
        zone: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into().trim().to_owned(),
            size_bytes,
            zone: zone.into().trim().to_owned(),
            project_id: project_id.into().trim().to_owned(),
            organisation_id: None,
            tier: VolumeTier::Sbs5k,
        }
    }

    /// Sets the optional organization identifier.
    #[must_use]
    pub fn organisation_id(mut self, value: Option<String>) -> Self {
        self.organisation_id = value.map(|id| id.trim().to_owned());
        self
    }

    /// Sets the performance tier.
    #[must_use]
    pub const fn with_tier(mut self, tier: VolumeTier) -> Self {
        self.tier = tier;
        self
    }

    /// Validates the provider-independent fields.
    ///
    /// # Errors
    ///
    /// Returns [`VolumeRequestError`] when a required field is blank, when the
    /// size is below 5 GB or above 15 TB, or when it is not a whole number of
    /// decimal gigabytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::volume::{VolumeRequest, VolumeRequestError};
    ///
    /// let small = VolumeRequest::new("cache", 1_000_000_000, "fr-par-1", "project");
    /// assert!(matches!(
    ///     small.validate(),
    ///     Err(VolumeRequestError::SizeTooSmall { .. })
    /// ));
    ///
    /// let valid = VolumeRequest::new("cache", 5_000_000_000, "fr-par-1", "project");
    /// assert!(valid.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), VolumeRequestError> {
        for (field, value) in [
            ("name", self.name.as_str()),
            ("zone", self.zone.as_str()),
            ("project_id", self.project_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(VolumeRequestError::MissingField {
                    field: field.to_owned(),
                });
            }
        }

        let minimum = MIN_VOLUME_SIZE_GB.saturating_mul(BYTES_PER_GB);
        let maximum = MAX_VOLUME_SIZE_GB.saturating_mul(BYTES_PER_GB);
        if self.size_bytes < minimum {
            return Err(VolumeRequestError::SizeTooSmall {
                size_bytes: self.size_bytes,
                minimum_bytes: minimum,
            });
        }
        if self.size_bytes > maximum {
            return Err(VolumeRequestError::SizeTooLarge {
                size_bytes: self.size_bytes,
                maximum_bytes: maximum,
            });
        }
        if !self.size_bytes.is_multiple_of(BYTES_PER_GB) {
            return Err(VolumeRequestError::SizeNotWholeGigabytes {
                size_bytes: self.size_bytes,
            });
        }
        Ok(())
    }
}

/// Errors raised when a [`VolumeRequest`] is not creation-ready.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VolumeRequestError {
    /// A required field was blank.
    #[error("volume request is missing {field}")]
    MissingField {
        /// Field that was blank.
        field: String,
    },
    /// The requested size is below the provider minimum.
    #[error(
        "volume size {size_bytes} bytes is below the 5 GB minimum of {minimum_bytes} bytes; increase volume_size_gb to at least 5"
    )]
    SizeTooSmall {
        /// Requested size in bytes.
        size_bytes: u64,
        /// Smallest accepted size in bytes.
        minimum_bytes: u64,
    },
    /// The requested size is above the documented maximum.
    #[error("volume size {size_bytes} bytes exceeds the 15 TB maximum of {maximum_bytes} bytes")]
    SizeTooLarge {
        /// Requested size in bytes.
        size_bytes: u64,
        /// Largest accepted size in bytes.
        maximum_bytes: u64,
    },
    /// The requested size is not a whole number of decimal gigabytes.
    #[error(
        "volume size {size_bytes} bytes is not a whole number of decimal gigabytes; express the size as a whole number of 1,000,000,000-byte units"
    )]
    SizeNotWholeGigabytes {
        /// Requested size in bytes.
        size_bytes: u64,
    },
}

/// Handle returned after creating a volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeHandle {
    /// Provider-specific volume identifier.
    pub id: String,
    /// Zone where the volume was created.
    pub zone: String,
}

/// Stage at which ordered teardown failed.
///
/// The stage tells an orchestrator whether the cache is known to be detached
/// (a destroy failure) or may still be attached (a stop or detach failure).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeardownStage {
    /// The instance could not be stopped.
    Stopping,
    /// The cache could not be detached.
    Detaching,
    /// The server could not be destroyed after detachment.
    Destroying,
}

/// A failed ordered teardown, tagged with the stage that failed.
#[derive(Debug)]
pub struct TeardownFailure<E> {
    /// Stage at which the failure occurred.
    pub stage: TeardownStage,
    /// Underlying backend error.
    pub source: E,
}

impl<E: fmt::Display> fmt::Display for TeardownFailure<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.source)
    }
}

impl<E: std::error::Error + 'static> std::error::Error for TeardownFailure<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { Some(&self.source) }
}

impl TeardownStage {
    /// Returns the stage as a short lower-case phrase.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopping => "stopping the server",
            Self::Detaching => "detaching the cache",
            Self::Destroying => "destroying the server",
        }
    }
}

/// Backend operations required for volume management.
pub trait VolumeBackend: Backend {
    /// Creates a new volume and returns its handle.
    fn create_volume<'a>(
        &'a self,
        request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error>;

    /// Reads one volume from the zone that holds it.
    fn get_volume<'a>(
        &'a self,
        zone: &'a str,
        volume_id: &'a str,
    ) -> BackendFuture<'a, VolumeState, Self::Error>;

    /// Polls until the volume reaches `status`, returning its final state.
    fn wait_for_volume_status<'a>(
        &'a self,
        zone: &'a str,
        volume_id: &'a str,
        status: VolumeStatus,
    ) -> BackendFuture<'a, VolumeState, Self::Error>;

    /// Deletes a volume that the current operation owns.
    fn delete_volume<'a>(&'a self, handle: &'a VolumeHandle) -> BackendFuture<'a, (), Self::Error>;

    /// Attaches a volume to a stopped instance and waits for `in_use`.
    fn attach_volume<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error>;

    /// Detaches a volume from a stopped instance and waits for `available`.
    fn detach_volume<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error>;

    /// Stops the instance if needed, detaches the volume when present, and
    /// destroys the server.
    ///
    /// The volume is left in `available` before the server is deleted, so the
    /// provider does not delete the cache with the server.
    fn teardown<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: Option<&'a str>,
    ) -> BackendFuture<'a, (), TeardownFailure<Self::Error>>;
}
