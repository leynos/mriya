//! Provider volume state, statuses and reference values.

use std::fmt;

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
    /// # Returns
    ///
    /// `true` for a `b_ssd` type or a `bssd` storage class.
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
    /// A status this client does not recognize.
    Unknown(String),
}

impl VolumeStatus {
    /// Maps a provider status string to the typed value.
    ///
    /// # Parameters
    ///
    /// - `value`: status string reported by the provider.
    ///
    /// # Returns
    ///
    /// A known status, or [`VolumeStatus::Unknown`] carrying the original
    /// value so a new provider state is not lost.
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
    ///
    /// # Returns
    ///
    /// The lower-case status string used by the API.
    #[must_use]
    pub const fn as_str(&self) -> &str {
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
    ///
    /// # Returns
    ///
    /// `true` only for [`VolumeStatus::Available`].
    #[must_use]
    pub const fn is_available(&self) -> bool { matches!(self, Self::Available) }

    /// Reports whether the volume is attached to a reference.
    ///
    /// # Returns
    ///
    /// `true` only for [`VolumeStatus::InUse`].
    #[must_use]
    pub const fn is_in_use(&self) -> bool { matches!(self, Self::InUse) }
}

impl fmt::Display for VolumeStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
