//! Rule-table tests for boot-volume plan resolution.
//!
//! Each row states one rule and one outcome, so a rule that never fires or
//! fires for the wrong reason fails a specific case rather than passing behind
//! a broad assertion. The table covers the accepting side of every rule as
//! well, which is what stops a "refuse everything" implementation passing.

use rstest::rstest;

use crate::{
    boot_volume::{
        BootVolumePlan,
        BootVolumeRequest,
        BootVolumeType,
        CompatibilityDefault,
        ImageStorage,
    },
    scaleway::{
        ScalewayBackendError,
        lifecycle::{
            boot_plan::{ImageFacts, SBS_DEFAULT_FLOOR_BYTES, resolve_boot_volume},
            products::{
                ConstraintSizes,
                ConstraintsByType,
                ProductCapabilities,
                ProductServerType,
            },
        },
    },
};

/// A 20–40 GB local range, for types that offer local storage.
fn local_range() -> ConstraintSizes {
    ConstraintSizes {
        min_size: 20_000_000_000,
        max_size: 40_000_000_000,
    }
}

/// A general 10–100 GB range, applied to every volume on the type.
fn general_range() -> ConstraintSizes {
    ConstraintSizes {
        min_size: 10_000_000_000,
        max_size: 100_000_000_000,
    }
}

/// Builds a catalogue entry with the chosen storage support.
fn product(
    arch: &str,
    general: ConstraintSizes,
    local: Option<ConstraintSizes>,
    block_storage: Option<bool>,
) -> ProductServerType {
    ProductServerType {
        volumes_constraint: general,
        per_volume_constraint: local.map(|sizes| ConstraintsByType { l_ssd: Some(sizes) }),
        arch: arch.to_owned(),
        end_of_service: false,
        capabilities: ProductCapabilities { block_storage },
    }
}

/// A type offering both local and Block Storage, with the standard ranges.
fn dual_product() -> ProductServerType {
    product("x86_64", general_range(), Some(local_range()), Some(true))
}

/// Image facts for an image whose root is `size_bytes`.
fn image(size_bytes: u64, storage: ImageStorage, volume_type: &'static str) -> ImageFacts<'static> {
    ImageFacts {
        size_bytes,
        storage,
        volume_type,
    }
}

/// A 10 GB local-only image.
fn local_image() -> ImageFacts<'static> {
    image(10_000_000_000, ImageStorage::new(true, false), "l_ssd")
}

/// A 10 GB Block-Storage-only image.
fn sbs_image() -> ImageFacts<'static> {
    image(
        10_000_000_000,
        ImageStorage::new(false, true),
        "sbs_snapshot",
    )
}

/// A 10 GB image that can back either storage type.
fn dual_image() -> ImageFacts<'static> {
    image(10_000_000_000, ImageStorage::new(true, true), "unified")
}

/// Resolves with the common architecture and a named instance type.
fn resolve(
    request: BootVolumeRequest,
    product: &ProductServerType,
    image: ImageFacts<'_>,
) -> Result<BootVolumePlan, ScalewayBackendError> {
    resolve_boot_volume("DEV1-S", &request, product, image, "x86_64")
}

/// A fully explicit plan inside every bound is accepted with no defaults.
#[rstest]
#[case::local(BootVolumeType::Local, 20_000_000_000)]
#[case::sbs(BootVolumeType::Sbs, 20_000_000_000)]
fn explicit_plan_inside_bounds_is_accepted(
    #[case] volume_type: BootVolumeType,
    #[case] size_bytes: u64,
) {
    let request = BootVolumeRequest {
        volume_type: Some(volume_type),
        size_gb: Some(20),
    };

    let plan = resolve(request, &dual_product(), dual_image()).expect("plan should resolve");

    assert_eq!(plan.volume_type, volume_type);
    assert_eq!(plan.size_bytes, size_bytes);
    assert_eq!(
        plan.compatibility_default,
        CompatibilityDefault::default(),
        "an explicit request must record no default"
    );
}

/// A size above the instance type's range is refused, naming the range.
#[test]
fn explicit_size_above_the_range_is_refused() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Local),
        size_gb: Some(41),
    };

    let error = resolve(request, &dual_product(), local_image())
        .expect_err("a size above the local maximum must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeSizeOutOfBounds {
                size_bytes: 41_000_000_000,
                min_size: 20_000_000_000,
                max_size: 40_000_000_000,
                ..
            }
        ),
        "{error:?}"
    );
}

/// A size below the instance type's range is refused.
#[test]
fn explicit_size_below_the_range_is_refused() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Local),
        size_gb: Some(19),
    };

    let error = resolve(request, &dual_product(), local_image())
        .expect_err("a size below the local minimum must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeSizeOutOfBounds {
                size_bytes: 19_000_000_000,
                ..
            }
        ),
        "{error:?}"
    );
}

/// A size smaller than the image cannot hold the operating system, even when
/// it is inside the instance type's range.
#[test]
fn explicit_size_below_the_image_is_refused() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Sbs),
        size_gb: Some(8),
    };
    let large_general = ConstraintSizes {
        min_size: 5_000_000_000,
        max_size: 100_000_000_000,
    };
    let catalogue = product("x86_64", large_general, None, Some(true));

    let error = resolve(request, &catalogue, sbs_image())
        .expect_err("a size below the image root must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeBelowImageSize {
                size_bytes: 8_000_000_000,
                image_size_bytes: 10_000_000_000,
                ..
            }
        ),
        "{error:?}"
    );
}

/// Requesting local storage on a type that offers none is refused, even when
/// the size would otherwise fit.
#[test]
fn local_request_on_a_type_without_local_storage_is_refused() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Local),
        size_gb: Some(20),
    };
    let catalogue = product("x86_64", general_range(), None, Some(true));

    let error = resolve(request, &catalogue, local_image())
        .expect_err("a type without local storage must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeTypeUnavailable {
                volume_type: BootVolumeType::Local,
                ..
            }
        ),
        "{error:?}"
    );
}

/// Requesting local storage on an SBS-only image is refused, naming the image's
/// own type.
#[test]
fn local_request_on_an_sbs_image_is_refused() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Local),
        size_gb: Some(20),
    };

    let error = resolve(request, &dual_product(), sbs_image())
        .expect_err("an SBS image cannot back a local boot volume");

    let ScalewayBackendError::BootVolumeTypeUnavailable { reason, .. } = &error else {
        panic!("expected a type-unavailable error, got {error:?}");
    };
    assert!(reason.contains("sbs_snapshot"), "{reason}");
}

/// Requesting Block Storage on a type that advertises none is refused.
#[test]
fn sbs_request_on_a_type_without_block_storage_is_refused() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Sbs),
        size_gb: Some(20),
    };
    let catalogue = product("x86_64", general_range(), Some(local_range()), Some(false));

    let error = resolve(request, &catalogue, sbs_image())
        .expect_err("a type without block storage must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeTypeUnavailable {
                volume_type: BootVolumeType::Sbs,
                ..
            }
        ),
        "{error:?}"
    );
}

/// A Block Storage size below the provider's own minimum is refused even when
/// the instance type would accept it.
#[test]
fn sbs_size_below_the_provider_minimum_is_refused() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Sbs),
        size_gb: Some(4),
    };
    let tiny_general = ConstraintSizes {
        min_size: 1_000_000_000,
        max_size: 100_000_000_000,
    };
    let catalogue = product("x86_64", tiny_general, None, Some(true));
    let tiny_image = image(
        1_000_000_000,
        ImageStorage::new(false, true),
        "sbs_snapshot",
    );

    let error = resolve(request, &catalogue, tiny_image)
        .expect_err("a size below the Block Storage minimum must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeSizeOutOfBounds {
                size_bytes: 4_000_000_000,
                min_size: 5_000_000_000,
                ..
            }
        ),
        "{error:?}"
    );
}

/// An unset type and size default to local at the type's maximum when both the
/// type and the image can do local, and both settings are recorded as
/// defaulted.
#[test]
fn unset_settings_default_to_local_maximum() {
    let plan = resolve(BootVolumeRequest::default(), &dual_product(), local_image())
        .expect("the compatibility default should resolve");

    assert_eq!(plan.volume_type, BootVolumeType::Local);
    assert_eq!(plan.size_bytes, 40_000_000_000, "local maximum");
    assert_eq!(
        plan.compatibility_default,
        CompatibilityDefault {
            volume_type: true,
            size: true,
        }
    );
}

/// An unset type defaults to Block Storage when the image cannot back local,
/// even on a type that offers local storage.
#[test]
fn unset_type_falls_back_to_sbs_for_an_sbs_image() {
    let plan = resolve(BootVolumeRequest::default(), &dual_product(), sbs_image())
        .expect("the compatibility default should resolve");

    assert_eq!(plan.volume_type, BootVolumeType::Sbs);
    assert_eq!(plan.size_bytes, SBS_DEFAULT_FLOOR_BYTES);
}

/// An unset Block Storage size defaults to the image's root size when that is
/// larger than the documented 10 GB floor.
#[test]
fn unset_sbs_size_defaults_to_the_image_when_larger() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Sbs),
        size_gb: None,
    };
    let bigger_image = image(30_000_000_000, ImageStorage::new(false, true), "sbs_volume");

    let plan =
        resolve(request, &dual_product(), bigger_image).expect("the size default should resolve");

    assert_eq!(plan.size_bytes, 30_000_000_000);
    assert_eq!(
        plan.compatibility_default,
        CompatibilityDefault {
            volume_type: false,
            size: true,
        },
        "only the size was defaulted"
    );
}

/// An unset local size defaults to the type's maximum rather than the image's
/// size, matching the provider's documented behaviour.
#[test]
fn unset_local_size_defaults_to_the_local_maximum() {
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Local),
        size_gb: None,
    };

    let plan =
        resolve(request, &dual_product(), local_image()).expect("the size default should resolve");

    assert_eq!(plan.size_bytes, 40_000_000_000);
    assert!(!plan.compatibility_default.volume_type);
    assert!(plan.compatibility_default.size);
}

/// A type offering a different architecture is refused before any size rule.
#[test]
fn architecture_mismatch_is_refused() {
    let catalogue = product("arm64", general_range(), Some(local_range()), Some(true));

    let error = resolve_boot_volume(
        "DEV1-S",
        &BootVolumeRequest::default(),
        &catalogue,
        local_image(),
        "x86_64",
    )
    .expect_err("an architecture mismatch must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeArchitectureMismatch {
                ref offered_arch,
                ref requested_arch,
                ..
            } if offered_arch == "arm64" && requested_arch == "x86_64"
        ),
        "{error:?}"
    );
}

/// A defaulted size taken from a catalogue maximum that is not 512-byte
/// aligned is refused rather than sent as a request the provider rejects.
#[test]
fn unaligned_default_is_refused() {
    let unaligned = ConstraintSizes {
        min_size: 20_000_000_000,
        max_size: 40_000_000_001,
    };
    let catalogue = product("x86_64", general_range(), Some(unaligned), Some(true));
    let request = BootVolumeRequest {
        volume_type: Some(BootVolumeType::Local),
        size_gb: None,
    };

    let error = resolve(request, &catalogue, local_image())
        .expect_err("an unaligned default must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeSizeUnaligned {
                size_bytes: 40_000_000_001,
                alignment: 512,
            }
        ),
        "{error:?}"
    );
}

/// Every decimal-GB size the resolver accepts is inherently 512-byte aligned,
/// which is why the alignment rule only ever fires for catalogue values.
#[rstest]
#[case::one_gb(1)]
#[case::twenty_gb(20)]
#[case::max_gb(u32::MAX)]
fn decimal_gb_sizes_are_aligned(#[case] size_gb: u32) {
    let bytes = crate::boot_volume::decimal_gb_to_bytes(size_gb).expect("convert");
    assert_eq!(bytes % 512, 0, "{size_gb} GB should be 512-byte aligned");
}
