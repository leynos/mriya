//! Pure boot-volume plan resolution.
//!
//! Resolution takes what the profile asked for, what the zone's product
//! catalogue says the instance type can do, and what the image's own root
//! volume looks like, and produces either a plan the provider will accept or
//! an error naming the rule that refused it. It is pure so the rule order can
//! be tested exhaustively without a provider.
//!
//! # Rule order
//!
//! 1. Architecture: the instance type must offer the requested architecture.
//! 2. Storage availability: the requested type must exist on that instance type, and the image must
//!    be able to back it.
//! 3. Size bounds: the size must cover the image's own root volume, and sit inside the instance
//!    type's range for that storage type.
//! 4. Alignment: the size must be a multiple of the provider's 512-byte increment.
//!
//! Rule order matters for the message a user sees: a local request against a
//! type with no local storage reports the missing storage, not a size bound.

use crate::{
    boot_volume::{
        BYTES_PER_DECIMAL_GB,
        BootVolumeError,
        BootVolumePlan,
        BootVolumeRequest,
        BootVolumeType,
        CompatibilityDefault,
        ImageStorage,
        SIZE_ALIGNMENT_BYTES,
        decimal_gb_to_bytes,
    },
    scaleway::{ScalewayBackendError, lifecycle::products::ProductServerType},
};

/// Smallest Block Storage volume Scaleway sells, in bytes.
///
/// Source: Scaleway Block Storage FAQ, which states the minimum Block Storage
/// volume is 5 GB and the maximum is 15 TB
/// (<https://www.scaleway.com/en/docs/block-storage/faq/>). Expressed in
/// decimal GB, the unit this backend uses throughout for boot storage.
pub(in crate::scaleway) const SBS_MIN_SIZE_BYTES: u64 = 5_000_000_000;

/// Largest Block Storage volume Scaleway sells, in bytes.
///
/// Source: the same FAQ page; 15 TB decimal is 15 * 10^12 bytes. This is a
/// provider-wide bound, independent of the instance type's own
/// `volumes_constraint`.
pub(in crate::scaleway) const SBS_MAX_SIZE_BYTES: u64 = 15_000_000_000_000;

/// Capacity a defaulted Block Storage boot volume will not go below, in bytes.
///
/// Source: Scaleway Instances API, "Creating an Instance: the volumes object",
/// which states that an image label defaults to an `sbs_volume` "the size of
/// the OS (typically 10GB in most cases)". Defaulting to at least 10 GB
/// reproduces the documented behaviour instead of producing a disk smaller
/// than the operating system it must hold.
pub(in crate::scaleway) const SBS_DEFAULT_FLOOR_BYTES: u64 = 10_000_000_000;

/// The facts resolution needs about the image being booted.
#[derive(Clone, Copy, Debug)]
pub(in crate::scaleway) struct ImageFacts<'a> {
    /// Size of the image's own root volume, in bytes.
    pub(in crate::scaleway) size_bytes: u64,
    /// Storage kinds the image can be booted from.
    pub(in crate::scaleway) storage: ImageStorage,
    /// Provider identifier of the image's root type, for error messages.
    pub(in crate::scaleway) volume_type: &'a str,
}

/// The size range that applies to one storage type on one instance type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SizeBounds {
    /// Smallest accepted size, in bytes.
    min_size: u64,
    /// Largest accepted size, in bytes.
    max_size: u64,
}

impl SizeBounds {
    /// Reports whether `size_bytes` is inside the inclusive range.
    const fn contains(&self, size_bytes: u64) -> bool {
        size_bytes >= self.min_size && size_bytes <= self.max_size
    }
}

/// The facts resolution needs about the instance type being requested.
#[derive(Clone, Copy, Debug)]
struct TypeFacts<'a> {
    /// The catalogue entry for the type.
    product: &'a ProductServerType,
}

impl TypeFacts<'_> {
    /// Reports whether the type offers local storage at all.
    const fn offers_local(&self) -> bool { self.product.local_constraint().is_some() }

    /// Reports whether the type can attach Block Storage.
    const fn offers_sbs(&self) -> bool { self.product.supports_block_storage() }

    /// Reports whether the type offers `volume_type`.
    const fn offers(&self, volume_type: BootVolumeType) -> bool {
        match volume_type {
            BootVolumeType::Local => self.offers_local(),
            BootVolumeType::Sbs => self.offers_sbs(),
        }
    }

    /// The size range that applies to `volume_type`.
    ///
    /// Local volumes use the published `per_volume_constraint.l_ssd`, which can
    /// differ from the general range. Block Storage uses the intersection of
    /// the type's general `volumes_constraint` and the provider-wide SBS range,
    /// because both must hold.
    fn bounds(&self, volume_type: BootVolumeType) -> Option<SizeBounds> {
        match volume_type {
            BootVolumeType::Local => self.product.local_constraint().map(|sizes| SizeBounds {
                min_size: sizes.min_size,
                max_size: sizes.max_size,
            }),
            BootVolumeType::Sbs => Some(SizeBounds {
                min_size: self
                    .product
                    .volumes_constraint
                    .min_size
                    .max(SBS_MIN_SIZE_BYTES),
                max_size: self
                    .product
                    .volumes_constraint
                    .max_size
                    .min(SBS_MAX_SIZE_BYTES),
            }),
        }
    }
}

/// Resolves a boot-volume plan from a profile request, a catalogue entry, and
/// the image's root facts.
///
/// `instance_type` is used only to name the type in errors.
///
/// # Errors
///
/// Returns the first rule that refuses the request, in the order documented at
/// the module level. Every refusal names the setting a user must change.
///
/// # Examples
///
/// ```ignore
/// // A fully explicit local plan inside every bound is accepted unchanged and
/// // records no compatibility default.
/// let plan = resolve_boot_volume(
///     "DEV1-S",
///     &request,
///     &product,
///     ImageFacts { size_bytes: 10_000_000_000, ..image },
///     "x86_64",
/// )?;
/// assert_eq!(plan.volume_type, BootVolumeType::Local);
/// assert!(!plan.compatibility_default.has_default());
/// ```
pub(in crate::scaleway) fn resolve_boot_volume(
    instance_type: &str,
    request: &BootVolumeRequest,
    product: &ProductServerType,
    image: ImageFacts<'_>,
    requested_architecture: &str,
) -> Result<BootVolumePlan, ScalewayBackendError> {
    let facts = TypeFacts { product };

    ensure_architecture(instance_type, product, requested_architecture)?;

    let (volume_type, type_was_defaulted) = choose_type(instance_type, request, &facts, &image)?;
    let size_bytes = choose_size(instance_type, request, &facts, &image, volume_type)?;

    ensure_aligned(size_bytes)?;

    Ok(BootVolumePlan::new(
        volume_type,
        size_bytes,
        CompatibilityDefault {
            volume_type: type_was_defaulted,
            size: request.size_gb.is_none(),
        },
    ))
}

/// Refuses a type whose published architecture is not the one requested.
///
/// A catalogue entry that omits `arch` is accepted: absence means the provider
/// did not say, not that it disagrees.
fn ensure_architecture(
    instance_type: &str,
    product: &ProductServerType,
    requested_architecture: &str,
) -> Result<(), ScalewayBackendError> {
    if product.arch.is_empty() || product.arch == requested_architecture {
        return Ok(());
    }
    Err(ScalewayBackendError::BootVolumeArchitectureMismatch {
        instance_type: instance_type.to_owned(),
        requested_arch: requested_architecture.to_owned(),
        offered_arch: product.arch.clone(),
    })
}

/// Picks the storage type, refusing an unavailable one and applying the
/// documented default when the profile named none.
///
/// Returns the chosen type and whether that type came from the default.
fn choose_type(
    instance_type: &str,
    request: &BootVolumeRequest,
    facts: &TypeFacts<'_>,
    image: &ImageFacts<'_>,
) -> Result<(BootVolumeType, bool), ScalewayBackendError> {
    if let Some(wanted) = request.volume_type {
        ensure_type_usable(instance_type, wanted, facts, image)?;
        return Ok((wanted, false));
    }

    // Documented default: local when the type offers local storage and the
    // image can be booted from it, which is what the provider does for a
    // marketplace image ID; Block Storage otherwise.
    let local_usable = facts.offers_local() && image.storage.local;
    let chosen = if local_usable {
        BootVolumeType::Local
    } else {
        BootVolumeType::Sbs
    };
    ensure_type_usable(instance_type, chosen, facts, image)?;
    Ok((chosen, true))
}

/// Refuses a storage type the instance type or the image cannot provide.
fn ensure_type_usable(
    instance_type: &str,
    wanted: BootVolumeType,
    facts: &TypeFacts<'_>,
    image: &ImageFacts<'_>,
) -> Result<(), ScalewayBackendError> {
    if !facts.offers(wanted) {
        return Err(ScalewayBackendError::BootVolumeTypeUnavailable {
            instance_type: instance_type.to_owned(),
            volume_type: wanted,
            reason: String::from("the instance type does not offer this storage"),
        });
    }
    if !image.storage.supports(wanted) {
        return Err(ScalewayBackendError::BootVolumeTypeUnavailable {
            instance_type: instance_type.to_owned(),
            volume_type: wanted,
            reason: format!(
                "image root type '{}' supports {}",
                image.volume_type,
                image.storage.describe()
            ),
        });
    }
    Ok(())
}

/// Picks the capacity, refusing one that cannot hold the image or that falls
/// outside the instance type's range.
fn choose_size(
    instance_type: &str,
    request: &BootVolumeRequest,
    facts: &TypeFacts<'_>,
    image: &ImageFacts<'_>,
    volume_type: BootVolumeType,
) -> Result<u64, ScalewayBackendError> {
    let size_bytes = match request.size_gb {
        Some(size_gb) => decimal_gb_to_bytes(size_gb).map_err(boot_volume_error)?,
        None => default_size(instance_type, facts, image, volume_type)?,
    };

    if size_bytes < image.size_bytes {
        return Err(ScalewayBackendError::BootVolumeBelowImageSize {
            instance_type: instance_type.to_owned(),
            volume_type,
            size_bytes,
            image_size_bytes: image.size_bytes,
        });
    }

    let bounds = facts.bounds(volume_type).ok_or_else(|| {
        ScalewayBackendError::BootVolumeTypeUnavailable {
            instance_type: instance_type.to_owned(),
            volume_type,
            reason: String::from("the instance type publishes no size range for it"),
        }
    })?;
    if !bounds.contains(size_bytes) {
        return Err(ScalewayBackendError::BootVolumeSizeOutOfBounds {
            instance_type: instance_type.to_owned(),
            volume_type,
            size_bytes,
            min_size: bounds.min_size,
            max_size: bounds.max_size,
        });
    }

    Ok(size_bytes)
}

/// Chooses the capacity used when the profile named none.
///
/// Local defaults to the type's maximum, which is what the provider does for a
/// marketplace image ID on a local-storage type. Block Storage defaults to the
/// larger of the image's root volume and [`SBS_DEFAULT_FLOOR_BYTES`], rounded
/// up to a whole decimal gigabyte and clamped into the accepted range.
fn default_size(
    instance_type: &str,
    facts: &TypeFacts<'_>,
    image: &ImageFacts<'_>,
    volume_type: BootVolumeType,
) -> Result<u64, ScalewayBackendError> {
    let bounds = facts.bounds(volume_type).ok_or_else(|| {
        ScalewayBackendError::BootVolumeTypeUnavailable {
            instance_type: instance_type.to_owned(),
            volume_type,
            reason: String::from("the instance type publishes no size range for it"),
        }
    })?;

    Ok(match volume_type {
        BootVolumeType::Local => bounds.max_size,
        BootVolumeType::Sbs => {
            let floor = image.size_bytes.max(SBS_DEFAULT_FLOOR_BYTES);
            round_up_to_decimal_gb(floor).clamp(bounds.min_size, bounds.max_size)
        }
    })
}

/// Rounds a byte size up to the next whole decimal gigabyte.
///
/// For example, 20_000_000_001 becomes 21_000_000_000 and an exact multiple is
/// unchanged.
const fn round_up_to_decimal_gb(size_bytes: u64) -> u64 {
    size_bytes.div_ceil(BYTES_PER_DECIMAL_GB) * BYTES_PER_DECIMAL_GB
}

/// Refuses a size the provider would reject as misaligned.
///
/// A whole decimal gigabyte is 10^9 bytes, which is 512 * 1_953_125, so every
/// size this backend constructs from whole gigabytes is already aligned. This
/// check exists for sizes that come from the catalogue instead, such as a
/// local default taken from a type's published maximum.
fn ensure_aligned(size_bytes: u64) -> Result<(), ScalewayBackendError> {
    if size_bytes % SIZE_ALIGNMENT_BYTES == 0 {
        return Ok(());
    }
    Err(ScalewayBackendError::BootVolumeSizeUnaligned {
        size_bytes,
        alignment: SIZE_ALIGNMENT_BYTES,
    })
}

/// Maps a domain conversion failure onto the backend's error type.
fn boot_volume_error(error: BootVolumeError) -> ScalewayBackendError {
    ScalewayBackendError::BootVolumeInvalid {
        message: error.to_string(),
    }
}
