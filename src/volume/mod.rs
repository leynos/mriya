//! Volume lifecycle abstractions for persistent cache storage.
//!
//! Scaleway sizes Block Storage volumes in decimal gigabytes (10^9 bytes) and
//! offers two performance tiers. The types here keep those facts in the port so
//! orchestrators do not have to know the provider's JSON shape. Creation,
//! lookup, readiness, attachment, detachment and deletion are all expressed as
//! backend operations so tests can substitute a scripted implementation.

use crate::backend::{Backend, BackendFuture, InstanceHandle};

/// Bytes in one decimal gigabyte (10^9), the granularity Scaleway uses for
/// Block Storage sizes.
pub const BYTES_PER_GB: u64 = 1_000_000_000;
/// Smallest Block Storage volume Scaleway accepts, in decimal gigabytes.
pub const MIN_VOLUME_SIZE_GB: u64 = 5;
/// Largest Block Storage volume Scaleway documents, in decimal gigabytes.
pub const MAX_VOLUME_SIZE_GB: u64 = 15_000;

mod state;
mod teardown;
mod tier;

pub use state::{VolumeReference, VolumeState, VolumeStatus};
pub use teardown::{TeardownFailure, TeardownStage};
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
    /// [`VolumeRequest::with_tier`] to request another. Construction does not
    /// validate the request.
    ///
    /// # Parameters
    ///
    /// - `name`: provider volume name.
    /// - `size_bytes`: decimal size in bytes, validated by [`Self::validate`].
    /// - `zone`: Availability Zone that will hold the volume.
    /// - `project_id`: project that owns the volume.
    ///
    /// # Returns
    ///
    /// The request with the default tier.
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
    ///
    /// # Parameters
    ///
    /// - `value`: organization identifier, trimmed when present.
    ///
    /// # Returns
    ///
    /// The request with the identifier replaced.
    #[must_use]
    pub fn organisation_id(mut self, value: Option<String>) -> Self {
        self.organisation_id = value.map(|id| id.trim().to_owned());
        self
    }

    /// Sets the performance tier.
    ///
    /// # Parameters
    ///
    /// - `tier`: Block Storage performance tier.
    ///
    /// # Returns
    ///
    /// The request with the tier replaced.
    #[must_use]
    pub const fn with_tier(mut self, tier: VolumeTier) -> Self {
        self.tier = tier;
        self
    }

    /// Validates the provider-independent fields.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the name and location are present and the size is a whole
    /// number of decimal gigabytes within the provider limits.
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

/// Backend operations required for volume management.
pub trait VolumeBackend: Backend {
    /// Creates a new volume.
    ///
    /// # Parameters
    ///
    /// - `request`: name, decimal size, zone, project and tier.
    ///
    /// # Returns
    ///
    /// A handle naming the new volume and its zone.
    ///
    /// # Errors
    ///
    /// The implementor rejects an invalid request, an unavailable tier or a
    /// provider failure before returning a handle.
    fn create_volume<'a>(
        &'a self,
        request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error>;

    /// Reads one volume from the zone that holds it.
    ///
    /// # Parameters
    ///
    /// - `zone`: Availability Zone to search.
    /// - `volume_id`: provider volume identifier.
    ///
    /// # Returns
    ///
    /// The provider's current state for that volume.
    ///
    /// # Errors
    ///
    /// Fails when the volume is absent from the zone or the provider rejects
    /// the read.
    fn get_volume<'a>(
        &'a self,
        zone: &'a str,
        volume_id: &'a str,
    ) -> BackendFuture<'a, VolumeState, Self::Error>;

    /// Polls until the volume reaches `status`.
    ///
    /// # Parameters
    ///
    /// - `zone`: Availability Zone that holds the volume.
    /// - `volume_id`: provider volume identifier.
    /// - `status`: terminal status to wait for.
    ///
    /// # Returns
    ///
    /// The final state, whose status equals the requested one.
    ///
    /// # Errors
    ///
    /// Fails on a terminal provider status, a lookup failure or the wait
    /// deadline.
    fn wait_for_volume_status<'a>(
        &'a self,
        zone: &'a str,
        volume_id: &'a str,
        status: VolumeStatus,
    ) -> BackendFuture<'a, VolumeState, Self::Error>;

    /// Deletes a volume that the current operation owns.
    ///
    /// # Parameters
    ///
    /// - `handle`: the volume created by this operation.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the volume is gone, including an already-absent volume.
    ///
    /// # Errors
    ///
    /// Fails when the provider rejects the deletion, such as an attached
    /// volume. Never pass a pre-existing volume here.
    fn delete_volume<'a>(&'a self, handle: &'a VolumeHandle) -> BackendFuture<'a, (), Self::Error>;

    /// Attaches a volume to a stopped instance and waits for `in_use`.
    ///
    /// # Parameters
    ///
    /// - `handle`: instance that will receive the volume.
    /// - `volume_id`: provider volume identifier.
    ///
    /// # Returns
    ///
    /// `Ok(())` after the returned volume map verifies and the status is
    /// `in_use`.
    ///
    /// # Errors
    ///
    /// A rejected request means nothing was attached. An unverifiable success
    /// response means the volume may be attached, so the caller must detach it
    /// before destroying the server.
    fn attach_volume<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error>;

    /// Detaches a volume from a stopped instance and waits for `available`.
    ///
    /// # Parameters
    ///
    /// - `handle`: instance that currently holds the volume.
    /// - `volume_id`: provider volume identifier.
    ///
    /// # Returns
    ///
    /// `Ok(())` after the cache is absent from the map and the status is
    /// `available`.
    ///
    /// # Errors
    ///
    /// Fails when the root or an unrelated entry is lost, the request is
    /// rejected, or the status never becomes available.
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
    ///
    /// # Parameters
    ///
    /// - `handle`: server to tear down.
    /// - `volume_id`: configured cache volume, or `None` for a plain destroy.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the server is gone.
    ///
    /// # Errors
    ///
    /// A [`TeardownFailure`] names the failed stage. A stop or detach failure
    /// means the volume may still be attached and the server is retained.
    fn teardown<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: Option<&'a str>,
    ) -> BackendFuture<'a, (), TeardownFailure<Self::Error>>;
}
