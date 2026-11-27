//! Defaults, architecture, and alignment rules for boot-volume resolution.
//!
//! These are the rules that apply when a profile leaves a setting out, or when
//! a value comes from the catalogue rather than the profile. They are split
//! from the main rule table so each file stays small and each test's intent
//! stays close to its neighbours.

use rstest::rstest;

use super::{
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
    scaleway::{
        ScalewayBackendError,
        lifecycle::{
            boot_plan::{Resolution, SBS_DEFAULT_FLOOR_BYTES, resolve_boot_volume},
            products::ConstraintSizes,
        },
    },
};

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
    let bigger_image = root_image(30_000_000_000, ImageStorage::new(false, true), "sbs_volume");

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

    let request = BootVolumeRequest::default();
    let error = resolve_boot_volume(Resolution {
        instance_type: "DEV1-S",
        request: &request,
        product: &catalogue,
        image: local_image(),
        requested_architecture: "x86_64",
    })
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
    assert!(
        bytes.is_multiple_of(512),
        "{size_gb} GB should be 512-byte aligned"
    );
}
