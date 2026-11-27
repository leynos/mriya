//! Decimal-gigabyte arithmetic and rendering for boot-volume sizes.
//!
//! Boot sizes are decimal gigabytes (10^9 bytes), the unit Scaleway's
//! published examples use. The cache volume's own size keeps its binary GiB
//! conversion; the two are deliberately not interchangeable, so these
//! conversions live together rather than in a shared "size" helper.

use super::BootVolumeError;

/// Bytes in one decimal gigabyte (10^9), the unit used for boot-volume sizes.
pub const BYTES_PER_DECIMAL_GB: u64 = 1_000_000_000;

/// Smallest volume increment the provider accepts, in bytes.
///
/// Scaleway's `VolumeServerTemplate.size` documentation states "Disk size of
/// the volume, must be a multiple of 512. (in bytes)".
pub const SIZE_ALIGNMENT_BYTES: u64 = 512;

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

/// Renders a byte size as both decimal gigabytes and raw bytes.
///
/// For example, `20_000_000_000` bytes renders as
/// `20 GB (20000000000 bytes)`, so a reader never has to guess the unit.
#[must_use]
pub fn format_size(size_bytes: u64) -> String {
    format!(
        "{} GB ({size_bytes} bytes)",
        size_bytes.div_ceil(BYTES_PER_DECIMAL_GB)
    )
}
