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

/// Performance tier of a Scaleway Block Storage volume.
///
/// Each tier maps to a provider `perf_iops` value and a provider type name.
///
/// # Examples
///
/// ```
/// use mriya::volume::VolumeTier;
///
/// assert_eq!(VolumeTier::Sbs5k.perf_iops(), 5_000);
/// assert_eq!(VolumeTier::Sbs15k.as_str(), "sbs_15k");
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VolumeTier {
    /// Block Storage 5K, the default tier.
    Sbs5k,
    /// Block Storage 15K, for higher-throughput workloads.
    Sbs15k,
}

impl VolumeTier {
    /// Returns the provider type name, such as `sbs_5k`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sbs5k => "sbs_5k",
            Self::Sbs15k => "sbs_15k",
        }
    }

    /// Returns the `perf_iops` value the provider expects.
    #[must_use]
    pub const fn perf_iops(self) -> u32 {
        match self {
            Self::Sbs5k => 5_000,
            Self::Sbs15k => 15_000,
        }
    }

    /// Parses a tier from its provider type name.
    ///
    /// # Errors
    ///
    /// Returns [`VolumeTierParseError`] for any value other than `sbs_5k` or
    /// `sbs_15k`, naming the accepted values.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::volume::VolumeTier;
    ///
    /// assert_eq!(
    ///     VolumeTier::parse(" sbs_15k ").expect("known tier"),
    ///     VolumeTier::Sbs15k
    /// );
    /// assert!(VolumeTier::parse("b_ssd").is_err());
    /// ```
    pub fn parse(value: &str) -> Result<Self, VolumeTierParseError> {
        match value.trim() {
            "sbs_5k" => Ok(Self::Sbs5k),
            "sbs_15k" => Ok(Self::Sbs15k),
            other => Err(VolumeTierParseError {
                value: other.to_owned(),
            }),
        }
    }
}

impl Default for VolumeTier {
    fn default() -> Self { Self::Sbs5k }
}

impl fmt::Display for VolumeTier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Error returned when a value is not a supported volume tier.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("unsupported volume tier '{value}'; use sbs_5k or sbs_15k")]
pub struct VolumeTierParseError {
    /// Value that was rejected.
    pub value: String,
}

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

/// Point-in-time view of a provider volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeState {
    /// Provider volume identifier.
    pub id: String,
    /// Zone the provider reports for the volume.
    pub zone: String,
    /// Project that owns the volume.
    pub project_id: String,
    /// Current provider status.
    pub status: VolumeStatus,
    /// Provider volume type, such as `sbs_5k` or the legacy `b_ssd`.
    pub volume_type: String,
    /// Storage class reported by the provider, such as `sbs` or `bssd`.
    pub storage_class: Option<String>,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Resources currently referencing the volume.
    pub references: Vec<VolumeReference>,
}

impl VolumeState {
    /// Reports whether the volume belongs to the legacy Block Storage class.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// assert!(legacy_volume.is_legacy());
    /// assert!(!sbs_volume.is_legacy());
    /// ```
    #[must_use]
    pub fn is_legacy(&self) -> bool {
        self.volume_type == "b_ssd"
            || self.volume_type.starts_with("b_ssd_")
            || self.storage_class.as_deref() == Some("bssd")
    }
}

/// A resource referencing a volume, such as an attached instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeReference {
    /// Reference identifier.
    pub id: String,
    /// Product resource type, such as `instance_server`.
    pub resource_type: String,
    /// Product resource identifier.
    pub resource_id: String,
    /// Reference status, such as `attached`.
    pub status: String,
}

impl VolumeReference {
    /// Renders `resource_type:resource_id (status)` for error messages.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{}:{} ({})",
            self.resource_type, self.resource_id, self.status
        )
    }
}

/// Provider volume status, tolerant of values this client does not know.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VolumeStatus {
    /// The volume is being created.
    Creating,
    /// The volume exists and is not attached.
    Available,
    /// The volume is attached to at least one reference.
    InUse,
    /// The volume is being deleted.
    Deleting,
    /// The volume has been deleted.
    Deleted,
    /// The volume is being resized.
    Resizing,
    /// The volume is in an error state.
    Error,
    /// The volume is being snapshotted.
    Snapshotting,
    /// The volume is locked.
    Locked,
    /// The volume is being updated.
    Updating,
    /// A status this client does not recognise.
    Unknown(String),
}

impl VolumeStatus {
    /// Maps a provider status string to the typed value.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::volume::VolumeStatus;
    ///
    /// assert_eq!(
    ///     VolumeStatus::from_provider("available"),
    ///     VolumeStatus::Available
    /// );
    /// assert_eq!(
    ///     VolumeStatus::from_provider("future_state"),
    ///     VolumeStatus::Unknown(String::from("future_state"))
    /// );
    /// ```
    #[must_use]
    pub fn from_provider(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "creating" => Self::Creating,
            "available" => Self::Available,
            "in_use" => Self::InUse,
            "deleting" => Self::Deleting,
            "deleted" => Self::Deleted,
            "resizing" => Self::Resizing,
            "error" => Self::Error,
            "snapshotting" => Self::Snapshotting,
            "locked" => Self::Locked,
            "updating" => Self::Updating,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Returns the provider spelling of the status.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Creating => "creating",
            Self::Available => "available",
            Self::InUse => "in_use",
            Self::Deleting => "deleting",
            Self::Deleted => "deleted",
            Self::Resizing => "resizing",
            Self::Error => "error",
            Self::Snapshotting => "snapshotting",
            Self::Locked => "locked",
            Self::Updating => "updating",
            Self::Unknown(value) => value.as_str(),
        }
    }

    /// Reports whether the volume is ready for attachment.
    #[must_use]
    pub fn is_available(&self) -> bool { matches!(self, Self::Available) }

    /// Reports whether the volume is attached to a reference.
    #[must_use]
    pub fn is_in_use(&self) -> bool { matches!(self, Self::InUse) }
}

impl fmt::Display for VolumeStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
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
    ) -> BackendFuture<'a, (), Self::Error>;
}
