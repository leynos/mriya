//! Boot-storage domain model shared by every backend.
//!
//! A boot volume is the disk an instance starts from, and it is separate from
//! any retained cache volume: the cache is attached for reuse across runs,
//! while the boot volume is scoped to one instance. Keeping the two apart in
//! the type system is what stops a cache volume ID being sent where a boot
//! specification belongs.
//!
//! Deleting an instance is *attempted* as part of teardown, but removal of the
//! boot volume is not yet guaranteed: Scaleway's own documentation disagrees
//! about whether a Block Storage root survives a server delete, and
//! ownership-aware cleanup is tracked in issue #154.
//!
//! Sizes here are decimal gigabytes (10^9 bytes), the unit Scaleway's
//! published examples use. The cache volume's own `mriya init` size keeps its
//! binary GiB conversion; that is a different setting with a different unit,
//! and the two are deliberately not interchangeable.

mod allocation;
mod measurement;
mod plan;
mod types;

use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

pub use allocation::{BootVolumeAllocation, BootVolumeMismatch, RootVolume};
pub use measurement::{
    BYTES_PER_DECIMAL_GB,
    SIZE_ALIGNMENT_BYTES,
    decimal_gb_to_bytes,
    format_size,
};
pub use plan::{BootVolumePlan, CompatibilityDefault};
use serde::{Deserialize, Serialize};
pub use types::{BootVolumeError, BootVolumeRequest, ImageStorage};

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

#[cfg(test)]
#[path = "../boot_volume_tests.rs"]
mod tests;
