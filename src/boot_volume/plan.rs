//! The resolved boot-volume plan and the record of which settings were
//! defaulted.
//!
//! A plan is the output of resolution: it names a storage type and an exact
//! byte size that every known constraint accepts, and it remembers which of
//! those came from the profile and which from the documented compatibility
//! default.

use super::{BYTES_PER_DECIMAL_GB, BootVolumeType};

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
    /// For example, `20_000_000_000` bytes yields 20 and a size one byte over
    /// that yields 21.
    #[must_use]
    pub fn size_gb(self) -> u32 {
        // Round up so a report never understates the purchase, and saturate
        // rather than wrap: a size this large is already an error elsewhere.
        let gb = self.size_bytes.div_ceil(BYTES_PER_DECIMAL_GB);
        u32::try_from(gb).unwrap_or(u32::MAX)
    }
}
