//! Rule-table tests for boot-volume plan resolution.
//!
//! Each row states one rule and one outcome, so a rule that never fires or
//! fires for the wrong reason fails a specific case rather than passing behind
//! a broad assertion. The table covers the accepting side of every rule as
//! well, which is what stops a "refuse everything" implementation passing.

use rstest::rstest;

use super::{
    dual_image,
    dual_product,
    general_range,
    local_image,
    local_range,
    product,
    resolve,
    root_image,
    sbs_image,
};
use crate::{
    boot_volume::{BootVolumeRequest, BootVolumeType, CompatibilityDefault, ImageStorage},
    scaleway::{ScalewayBackendError, lifecycle::products::ConstraintSizes},
};

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
    let tiny_image = root_image(
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
