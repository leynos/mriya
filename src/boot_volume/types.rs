//! Storage types, validation errors, and the request a profile states.
//!
//! These are the pieces a caller names directly when configuring a boot
//! volume; the resolved plan and the reported allocation live in sibling
//! modules.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{BootVolumeType, SIZE_ALIGNMENT_BYTES};

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
