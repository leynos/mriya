//! Boot-storage domain model shared by every backend.
//!
//! A boot volume is the disk an instance starts from, and it is separate
//! from any retained cache volume: the cache is attached for reuse across
//! runs, while the boot volume is destroyed with the instance. Keeping the
//! two apart in the type system is what stops a cache volume ID being sent
//! where a boot specification belongs.
//!
//! Sizes here are decimal gigabytes (10^9 bytes), the unit Scaleway's
//! published examples use. The cache volume's own `mriya init` size keeps its
//! binary GiB conversion; that is a different setting with a different unit,
//! and the two are deliberately not interchangeable.

use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Bytes in one decimal gigabyte (10^9), the unit used for boot-volume sizes.
pub const BYTES_PER_DECIMAL_GB: u64 = 1_000_000_000;

/// Smallest volume increment the provider accepts, in bytes.
///
/// Scaleway's `VolumeServerTemplate.size` documentation states "Disk size of
/// the volume, must be a multiple of 512. (in bytes)".
pub const SIZE_ALIGNMENT_BYTES: u64 = 512;

/// Supported boot-volume storage types, named by their provider identifiers.
///
/// The identifiers are the values Scaleway's `Volume.VolumeType` enum accepts
/// for a bootable volume: `l_ssd` for local SSD and `sbs_volume` for Block
/// Storage.
///
/// # Examples
///
/// ```ignore
/// // The provider identifier round-trips through parse and display.
/// let local: BootVolumeType = "l_ssd".parse()?;
/// assert_eq!(local.to_string(), "l_ssd");
/// let sbs: BootVolumeType = "sbs_volume".parse()?;
/// assert_eq!(sbs.to_string(), "sbs_volume");
///
/// // An unknown identifier is refused rather than guessed at.
/// assert!("b_ssd".parse::<BootVolumeType>().is_err());
/// ```
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum BootVolumeType {
    /// Local SSD, provider identifier `l_ssd`.
    #[serde(rename = "l_ssd")]
    Local,
    /// Block Storage, provider identifier `sbs_volume`.
    #[serde(rename = "sbs_volume")]
    Sbs,
}

impl BootVolumeType {
    /// The provider's identifier for this storage type.
    ///
    /// For example, [`BootVolumeType::Local`] yields `l_ssd`.
    #[must_use]
    pub const fn provider_id(self) -> &'static str {
        match self {
            Self::Local => "l_ssd",
            Self::Sbs => "sbs_volume",
        }
    }

    /// A short human label used in reports, such as `local` or `sbs`.
    ///
    /// For example, [`BootVolumeType::Local`] yields `local`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Sbs => "sbs",
        }
    }
}

impl Display for BootVolumeType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.provider_id())
    }
}

impl FromStr for BootVolumeType {
    type Err = BootVolumeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "l_ssd" => Ok(Self::Local),
            "sbs_volume" => Ok(Self::Sbs),
            other => Err(BootVolumeError::UnknownType(other.to_owned())),
        }
    }
}

/// The storage kinds an image can be booted from.
///
/// A marketplace image records which volume type it was captured from. An
/// `l_ssd` image can only back a local boot volume, an `sbs_snapshot` or
/// `sbs_volume` image can only back Block Storage, and a `unified` image can
/// back either.
///
/// # Examples
///
/// ```ignore
/// let local_only = ImageStorage::new(true, false);
/// assert!(local_only.supports(BootVolumeType::Local));
/// assert!(!local_only.supports(BootVolumeType::Sbs));
///
/// let either = ImageStorage::new(true, true);
/// assert!(either.supports(BootVolumeType::Sbs));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageStorage {
    /// True when the image can back a local SSD boot volume.
    pub local: bool,
    /// True when the image can back a Block Storage boot volume.
    pub sbs: bool,
}

impl ImageStorage {
    /// Creates a storage-compatibility pair.
    #[must_use]
    pub const fn new(local: bool, sbs: bool) -> Self { Self { local, sbs } }

    /// Reports whether the image can back `volume_type`.
    ///
    /// For example, a Block-Storage-only image rejects a local plan.
    #[must_use]
    pub const fn supports(self, volume_type: BootVolumeType) -> bool {
        match volume_type {
            BootVolumeType::Local => self.local,
            BootVolumeType::Sbs => self.sbs,
        }
    }

    /// The supported types, as report text.
    ///
    /// For example, an SBS-only image renders `sbs_volume`.
    #[must_use]
    pub fn describe(self) -> String {
        match (self.local, self.sbs) {
            (true, true) => String::from("l_ssd, sbs_volume"),
            (true, false) => String::from("l_ssd"),
            (false, true) => String::from("sbs_volume"),
            (false, false) => String::from("none"),
        }
    }
}

/// Errors raised while parsing or validating boot-storage settings.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BootVolumeError {
    /// Raised when a storage type is not one this backend can request.
    #[error("unknown boot volume type '{0}': supported types are l_ssd and sbs_volume")]
    UnknownType(String),
    /// Raised when a configured size is zero.
    #[error(
        "boot volume size must be greater than zero: set SCW_BOOT_VOLUME_SIZE_GB \
         or add boot_volume_size_gb to [scaleway] in mriya.toml"
    )]
    ZeroSize,
    /// Raised when a size in decimal gigabytes does not fit in a `u64`.
    #[error("boot volume size {size_gb} GB is too large to represent in bytes")]
    SizeOverflow {
        /// The size that could not be converted.
        size_gb: u32,
    },
    /// Raised when a computed byte size is not a multiple of the provider's
    /// 512-byte increment.
    #[error("boot volume size {size_bytes} bytes is not a multiple of {SIZE_ALIGNMENT_BYTES}")]
    UnalignedSize {
        /// The size that failed alignment.
        size_bytes: u64,
    },
}

impl BootVolumeError {
    /// Wording for a zero size that names the environment variable and key.
    ///
    /// The caller supplies the variable and key names so the same error reads
    /// correctly whether it came from the profile or the command line.
    ///
    /// For example, `describe_zero_size("SCW_BOOT_VOLUME_SIZE_GB",
    /// "boot_volume_size_gb")` yields a message naming both.
    #[must_use]
    pub fn describe_zero_size(env_var: &str, toml_key: &str) -> String {
        format!(
            "boot volume size must be greater than zero: set {env_var} or add \
             {toml_key} to [scaleway] in mriya.toml"
        )
    }
}

/// A profile's boot-storage request, before provider constraints are known.
///
/// Either setting may be absent, in which case the backend chooses a
/// documented default and records that it did so on the resulting
/// [`BootVolumePlan`].
///
/// # Examples
///
/// ```ignore
/// // Both settings may be named explicitly.
/// let request = BootVolumeRequest {
///     volume_type: Some(BootVolumeType::Sbs),
///     size_gb: Some(20),
/// };
///
/// // An empty request asks for the documented compatibility default.
/// let request = BootVolumeRequest::default();
/// assert!(request.is_unset());
/// ```
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BootVolumeRequest {
    /// Requested storage type, or `None` to accept the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_type: Option<BootVolumeType>,
    /// Requested capacity in decimal gigabytes, or `None` for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_gb: Option<u32>,
}

impl BootVolumeRequest {
    /// Creates a request with both settings named.
    #[must_use]
    pub const fn new(volume_type: BootVolumeType, size_gb: u32) -> Self {
        Self {
            volume_type: Some(volume_type),
            size_gb: Some(size_gb),
        }
    }

    /// Reports whether the profile named neither setting.
    ///
    /// For example, [`BootVolumeRequest::default`] is unset while a request
    /// carrying only a type is not.
    #[must_use]
    pub const fn is_unset(&self) -> bool { self.volume_type.is_none() && self.size_gb.is_none() }

    /// Validates that any supplied size is non-zero.
    ///
    /// # Errors
    ///
    /// Returns [`BootVolumeError::ZeroSize`] when the size is zero.
    pub const fn validate(&self) -> Result<(), BootVolumeError> {
        match self.size_gb {
            Some(0) => Err(BootVolumeError::ZeroSize),
            _ => Ok(()),
        }
    }
}

/// Which settings a [`BootVolumePlan`] took from the provider default.
///
/// An empty value means every setting came from the profile.
///
/// # Examples
///
/// ```ignore
/// // A fully explicit plan records nothing as defaulted.
/// assert!(!CompatibilityDefault::default().has_default());
///
/// // A size-only request marks the type as defaulted.
/// let defaulted = CompatibilityDefault {
///     volume_type: true,
///     size: false,
/// };
/// assert!(defaulted.has_default());
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompatibilityDefault {
    /// True when the profile named no storage type.
    pub volume_type: bool,
    /// True when the profile named no capacity.
    pub size: bool,
}

impl CompatibilityDefault {
    /// Reports whether any setting came from the compatibility default.
    ///
    /// For example, a plan defaulted only in size yields `true`.
    #[must_use]
    pub const fn has_default(self) -> bool { self.volume_type || self.size }

    /// The settings that were defaulted, as report text.
    ///
    /// For example, a type-only default renders `type` and both defaults
    /// render `type, size`.
    #[must_use]
    pub fn describe(self) -> String {
        match (self.volume_type, self.size) {
            (true, true) => String::from("type, size"),
            (true, false) => String::from("type"),
            (false, true) => String::from("size"),
            (false, false) => String::new(),
        }
    }
}

/// A boot-volume choice that satisfies every known constraint.
///
/// # Examples
///
/// ```ignore
/// let plan = BootVolumePlan {
///     volume_type: BootVolumeType::Local,
///     size_bytes: 20_000_000_000,
///     compatibility_default: CompatibilityDefault::default(),
/// };
/// assert_eq!(plan.size_bytes, 20 * BYTES_PER_DECIMAL_GB);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootVolumePlan {
    /// Storage type the create request will ask for.
    pub volume_type: BootVolumeType,
    /// Capacity in bytes, aligned to [`SIZE_ALIGNMENT_BYTES`].
    pub size_bytes: u64,
    /// Which settings came from the compatibility default.
    pub compatibility_default: CompatibilityDefault,
}

impl BootVolumePlan {
    /// Creates a plan from a validated type and byte size.
    ///
    /// For example, a 20 GB local plan reports `size_gb()` as 20.
    #[must_use]
    pub const fn new(
        volume_type: BootVolumeType,
        size_bytes: u64,
        compatibility_default: CompatibilityDefault,
    ) -> Self {
        Self {
            volume_type,
            size_bytes,
            compatibility_default,
        }
    }

    /// The capacity in whole decimal gigabytes, rounded up.
    ///
    /// For example, 20_000_000_000 bytes yields 20 and a size one byte over
    /// that yields 21.
    #[must_use]
    pub const fn size_gb(self) -> u32 {
        // Round up so a report never understates the purchase.
        let gb = self.size_bytes.div_ceil(BYTES_PER_DECIMAL_GB);
        if gb > u32::MAX as u64 {
            u32::MAX
        } else {
            gb as u32
        }
    }
}

/// Converts a capacity in decimal gigabytes to bytes, checked for overflow.
///
/// # Errors
///
/// Returns [`BootVolumeError::SizeOverflow`] when the product exceeds `u64`.
///
/// # Examples
///
/// ```ignore
/// // Ten decimal gigabytes is ten thousand million bytes.
/// assert_eq!(decimal_gb_to_bytes(10)?, 10_000_000_000);
/// // No u32 value overflows a u64, so the check documents the contract.
/// assert_eq!(decimal_gb_to_bytes(u32::MAX)?.to_string(), "4294967295000000000");
/// ```
pub fn decimal_gb_to_bytes(size_gb: u32) -> Result<u64, BootVolumeError> {
    u64::from(size_gb)
        .checked_mul(BYTES_PER_DECIMAL_GB)
        .ok_or(BootVolumeError::SizeOverflow { size_gb })
}

/// Rounds a byte size up to the provider's 512-byte increment.
///
/// For example, 513 bytes becomes 1024 and an already aligned size is
/// unchanged.
#[must_use]
pub const fn align_up_to_size_increment(size_bytes: u64) -> u64 {
    size_bytes.div_ceil(SIZE_ALIGNMENT_BYTES) * SIZE_ALIGNMENT_BYTES
}

/// Renders a byte size as both decimal gigabytes and raw bytes.
///
/// For example, 20_000_000_000 bytes renders as
/// `20 GB (20000000000 bytes)`, so a reader never has to guess the unit.
#[must_use]
pub fn format_size(size_bytes: u64) -> String {
    format!(
        "{} GB ({size_bytes} bytes)",
        size_bytes.div_ceil(BYTES_PER_DECIMAL_GB)
    )
}

/// The root volume an instance actually reports after creation.
///
/// This is the provider's answer, not the request: it is what the run
/// actually paid for, and what issue #154 needs in order to clean up.
///
/// # Examples
///
/// ```ignore
/// let root = RootVolume {
///     id: String::from("vol-root"),
///     volume_type: BootVolumeType::Local,
///     size_bytes: 20_000_000_000,
/// };
/// assert_eq!(root.volume_type.provider_id(), "l_ssd");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootVolume {
    /// Provider identifier of the root volume.
    pub id: String,
    /// Provider-reported storage type.
    pub volume_type: BootVolumeType,
    /// Provider-reported capacity in bytes.
    pub size_bytes: u64,
}

/// The boot volume a created instance ended up with.
///
/// The handle carries this so later stages can report and clean up the real
/// allocation rather than re-deriving it from the request.
///
/// # Examples
///
/// ```ignore
/// let allocation = BootVolumeAllocation {
///     plan: BootVolumePlan::new(BootVolumeType::Local, 20_000_000_000, Default::default()),
///     root: RootVolume { id: String::from("vol-root"), ..root },
/// };
/// assert_eq!(allocation.root.id, "vol-root");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootVolumeAllocation {
    /// The plan the create request asked for.
    pub plan: BootVolumePlan,
    /// The root volume the provider reported.
    pub root: RootVolume,
}

/// Errors raised when a reported root volume does not match the plan.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BootVolumeMismatch {
    /// Raised when the provider reports no bootable root volume at all.
    #[error("instance {instance_id} reported no bootable root volume")]
    Missing {
        /// Instance whose root volume was missing.
        instance_id: String,
    },
    /// Raised when the reported type or size differs from the plan.
    #[error(
        "instance {instance_id} root volume {volume_id} is {actual}, but the plan \
         requested {expected}"
    )]
    Differs {
        /// Instance holding the root volume.
        instance_id: String,
        /// Provider identifier of the root volume.
        volume_id: String,
        /// What the plan asked for, already formatted.
        expected: String,
        /// What the provider reported, already formatted.
        actual: String,
    },
}

impl BootVolumeMismatch {
    /// Builds the mismatch for a reported root volume that differs from
    /// `plan`.
    ///
    /// For example, a local plan answered by an SBS root volume yields
    /// [`BootVolumeMismatch::Differs`] naming both.
    #[must_use]
    pub fn differs(instance_id: &str, root: &RootVolume, plan: &BootVolumePlan) -> Self {
        Self::Differs {
            instance_id: instance_id.to_owned(),
            volume_id: root.id.clone(),
            expected: format!(
                "{} {}",
                plan.volume_type.provider_id(),
                format_size(plan.size_bytes)
            ),
            actual: format!(
                "{} {}",
                root.volume_type.provider_id(),
                format_size(root.size_bytes)
            ),
        }
    }
}

impl BootVolumeAllocation {
    /// Verifies that `root` is the volume `plan` asked for.
    ///
    /// # Errors
    ///
    /// Returns [`BootVolumeMismatch::Differs`] when the type or size differs.
    ///
    /// For example, a plan for a 20 GB `l_ssd` accepts exactly that root and
    /// rejects a 21 GB one.
    pub fn verify(
        instance_id: &str,
        plan: &BootVolumePlan,
        root: RootVolume,
    ) -> Result<Self, BootVolumeMismatch> {
        if root.volume_type != plan.volume_type || root.size_bytes != plan.size_bytes {
            return Err(BootVolumeMismatch::differs(instance_id, &root, plan));
        }
        Ok(Self { plan: *plan, root })
    }
}

#[cfg(test)]
#[path = "boot_volume_tests.rs"]
mod tests;
