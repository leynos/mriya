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
    boot_volume::{
        BootVolumePlan,
        BootVolumeRequest,
        BootVolumeType,
        CompatibilityDefault,
        ImageStorage,
    },
    scaleway::{ScalewayBackendError, lifecycle::products::ConstraintSizes},
};

/// Builds a request that names both settings.
fn request(volume_type: BootVolumeType, size_gb: u32) -> BootVolumeRequest {
    BootVolumeRequest {
        volume_type: Some(volume_type),
        size_gb: Some(size_gb),
    }
}

/// Resolves a request that must be refused and returns the refusal.
///
/// For example, a 41 GB local request against a 20-40 GB range yields the
/// bounds error rather than a plan.
fn refusal(
    request: BootVolumeRequest,
    product: &crate::scaleway::lifecycle::products::ProductServerType,
    image: crate::scaleway::lifecycle::boot_plan::ImageFacts<'_>,
) -> ScalewayBackendError {
    resolve(request, product, image).expect_err("the request should have been refused")
}

/// The rule a refusal names, reduced to the values the tests compare.
///
/// Classifying once keeps every case to one assertion, and keeps the
/// comparison tables readable without a helper per error variant.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Refusal {
    /// The size fell outside the accepted range.
    OutOfBounds {
        /// The size that was refused, in bytes.
        size_bytes: u64,
    },
    /// The storage type is not available for this instance type or image.
    TypeUnavailable {
        /// The storage type that was refused.
        volume_type: BootVolumeType,
    },
    /// Any other refusal, named by its rendered message.
    Other(String),
}

impl Refusal {
    /// Classifies `error`.
    ///
    /// For example, a bounds refusal yields
    /// [`Refusal::OutOfBounds`] carrying the refused size.
    fn of(error: &ScalewayBackendError) -> Self {
        match error {
            ScalewayBackendError::BootVolumeSizeOutOfBounds { size_bytes, .. } => {
                Self::OutOfBounds {
                    size_bytes: *size_bytes,
                }
            }
            ScalewayBackendError::BootVolumeTypeUnavailable { volume_type, .. } => {
                Self::TypeUnavailable {
                    volume_type: *volume_type,
                }
            }
            other => Self::Other(other.to_string()),
        }
    }
}

/// Asserts that `error` is the refusal described by `expected`.
fn assert_refusal(error: &ScalewayBackendError, expected: &Refusal) {
    assert_eq!(
        Refusal::of(error),
        *expected,
        "unexpected refusal: {error:?}"
    );
}

/// A fully explicit plan inside every bound is accepted with no defaults.
#[rstest]
#[case::local(BootVolumeType::Local, 20_000_000_000)]
#[case::sbs(BootVolumeType::Sbs, 20_000_000_000)]
fn explicit_plan_inside_bounds_is_accepted(
    #[case] volume_type: BootVolumeType,
    #[case] size_bytes: u64,
) {
    let plan = resolve(request(volume_type, 20), &dual_product(), dual_image())
        .expect("plan should resolve");

    assert_eq!(plan.volume_type, volume_type);
    assert_eq!(plan.size_bytes, size_bytes);
    assert_eq!(
        plan.compatibility_default,
        CompatibilityDefault::default(),
        "an explicit request must record no default"
    );
}

/// Each refusal row states one rule and the refusal it must name.
///
/// The rows share a shape — a request, a catalogue entry, an image, and the
/// expected refusal — so they are one table rather than four near-identical
/// tests. A rule that fires for the wrong case fails its own row.
#[rstest]
#[case::above_range(
    request(BootVolumeType::Local, 41),
    dual_product(),
    local_image(),
    Refusal::OutOfBounds { size_bytes: 41_000_000_000 },
)]
#[case::below_range(
    request(BootVolumeType::Local, 19),
    dual_product(),
    local_image(),
    Refusal::OutOfBounds { size_bytes: 19_000_000_000 },
)]
#[case::no_local_storage(
    request(BootVolumeType::Local, 20),
    product("x86_64", general_range(), None, Some(true)),
    local_image(),
    Refusal::TypeUnavailable { volume_type: BootVolumeType::Local },
)]
#[case::no_block_storage(
    request(BootVolumeType::Sbs, 20),
    product("x86_64", general_range(), Some(local_range()), Some(false)),
    sbs_image(),
    Refusal::TypeUnavailable { volume_type: BootVolumeType::Sbs },
)]
fn a_refused_request_names_its_rule(
    #[case] request: BootVolumeRequest,
    #[case] product: crate::scaleway::lifecycle::products::ProductServerType,
    #[case] image: crate::scaleway::lifecycle::boot_plan::ImageFacts<'static>,
    #[case] expected: Refusal,
) {
    let error = refusal(request, &product, image);

    assert_refusal(&error, &expected);
}

/// A size smaller than the image cannot hold the operating system, even when
/// it is inside the instance type's range.
#[test]
fn explicit_size_below_the_image_is_refused() {
    let large_general = ConstraintSizes {
        min_size: 5_000_000_000,
        max_size: 100_000_000_000,
    };
    let catalogue = product("x86_64", large_general, None, Some(true));

    let error = refusal(request(BootVolumeType::Sbs, 8), &catalogue, sbs_image());

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

/// Requesting local storage on an SBS-only image is refused, naming the image's
/// own type.
#[test]
fn local_request_on_an_sbs_image_is_refused() {
    let error = refusal(
        request(BootVolumeType::Local, 20),
        &dual_product(),
        sbs_image(),
    );

    let ScalewayBackendError::BootVolumeTypeUnavailable { reason, .. } = &error else {
        panic!("expected a type-unavailable error, got {error:?}");
    };
    assert!(reason.contains("sbs_snapshot"), "{reason}");
}

/// A Block Storage size below the provider's own minimum is refused even when
/// the instance type would accept it.
#[test]
fn sbs_size_below_the_provider_minimum_is_refused() {
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

    let error = refusal(request(BootVolumeType::Sbs, 4), &catalogue, tiny_image);

    assert_refusal(
        &error,
        &Refusal::OutOfBounds {
            size_bytes: 4_000_000_000,
        },
    );
}

/// A plan records the resolved settings so a report can render them.
#[test]
fn a_resolved_plan_reports_its_type_and_size() {
    let plan: BootVolumePlan = resolve(
        request(BootVolumeType::Local, 20),
        &dual_product(),
        local_image(),
    )
    .expect("plan should resolve");

    assert_eq!(plan.volume_type.provider_id(), "l_ssd");
    assert_eq!(plan.size_bytes, 20_000_000_000);
}
