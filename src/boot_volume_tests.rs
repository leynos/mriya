//! Unit tests for the boot-storage domain model.

use rstest::rstest;

use super::*;

/// Every supported type round-trips through `FromStr` and `Display`.
#[rstest]
#[case::local("l_ssd", BootVolumeType::Local, "l_ssd", "local")]
#[case::sbs("sbs_volume", BootVolumeType::Sbs, "sbs_volume", "sbs")]
fn boot_volume_type_round_trips(
    #[case] input: &str,
    #[case] expected: BootVolumeType,
    #[case] provider_id: &str,
    #[case] label: &str,
) {
    let parsed: BootVolumeType = input.parse().expect("supported type should parse");
    assert_eq!(parsed, expected);
    assert_eq!(parsed.provider_id(), provider_id);
    assert_eq!(parsed.to_string(), provider_id);
    assert_eq!(parsed.label(), label);
}

/// Parsing trims surrounding whitespace before matching.
#[test]
fn boot_volume_type_parse_trims_whitespace() {
    assert_eq!(
        "  sbs_volume ".parse::<BootVolumeType>(),
        Ok(BootVolumeType::Sbs)
    );
}

/// Provider identifiers this backend cannot request are refused by name.
#[rstest]
#[case::deprecated_block("b_ssd")]
#[case::scratch("scratch")]
#[case::unified("unified")]
#[case::snapshot("sbs_snapshot")]
#[case::empty("")]
#[case::unknown("block")]
fn boot_volume_type_rejects_unsupported_identifiers(#[case] input: &str) {
    let error = input
        .parse::<BootVolumeType>()
        .expect_err("unsupported type must be refused");
    assert_eq!(error, BootVolumeError::UnknownType(input.to_owned()));
    // The message names the supported alternatives so a user can correct it.
    assert!(
        error.to_string().contains("l_ssd") && error.to_string().contains("sbs_volume"),
        "{error}"
    );
}

/// Serde uses the provider identifiers verbatim, so configuration files and
/// provider payloads agree.
#[test]
fn boot_volume_type_serde_uses_provider_identifiers() {
    assert_eq!(
        serde_json::to_string(&BootVolumeType::Local).expect("serialize"),
        r#""l_ssd""#
    );
    assert_eq!(
        serde_json::to_string(&BootVolumeType::Sbs).expect("serialize"),
        r#""sbs_volume""#
    );
    assert_eq!(
        serde_json::from_str::<BootVolumeType>(r#""l_ssd""#).expect("deserialize"),
        BootVolumeType::Local
    );
}

/// The decimal conversion is exact and states its unit.
#[rstest]
#[case::one(1, 1_000_000_000)]
#[case::ten(10, 10_000_000_000)]
#[case::twenty(20, 20_000_000_000)]
#[case::max(u32::MAX, 4_294_967_295_000_000_000)]
fn decimal_gb_to_bytes_is_exact(#[case] size_gb: u32, #[case] expected: u64) {
    let bytes = decimal_gb_to_bytes(size_gb).expect("every u32 fits in u64 bytes");
    assert_eq!(bytes, expected);
}

/// The conversion is decimal, not binary, so a GB is 10^9 bytes and never the
/// 2^30 a GiB would be.
#[test]
fn decimal_gb_is_not_a_gibibyte() {
    let decimal = decimal_gb_to_bytes(10).expect("convert");
    assert_eq!(decimal, 10_000_000_000);
    assert_ne!(decimal, 10 * 1024 * 1024 * 1024);
}

/// Size rendering states decimal GB and bytes together.
#[rstest]
#[case::ten(10_000_000_000, "10 GB (10000000000 bytes)")]
#[case::partial(10_000_000_001, "11 GB (10000000001 bytes)")]
fn format_size_states_both_units(#[case] size_bytes: u64, #[case] expected: &str) {
    assert_eq!(format_size(size_bytes), expected);
}

/// A zero size is refused, naming the variable and key a user must change.
#[test]
fn boot_volume_request_rejects_zero_size() {
    let request = BootVolumeRequest::new(BootVolumeType::Local, 0);
    assert_eq!(request.validate(), Err(BootVolumeError::ZeroSize));
    let message = BootVolumeError::describe_zero_size("SCW_X_TYPE", "x_type");
    assert!(message.contains("SCW_X_TYPE"), "{message}");
    assert!(message.contains("x_type"), "{message}");
}

/// Unset detection distinguishes an empty request from a partial one.
#[rstest]
#[case::unset(BootVolumeRequest::default(), true)]
#[case::type_only(
    BootVolumeRequest { volume_type: Some(BootVolumeType::Local), size_gb: None },
    false,
)]
#[case::size_only(
    BootVolumeRequest { volume_type: None, size_gb: Some(10) },
    false,
)]
#[case::both(BootVolumeRequest::new(BootVolumeType::Sbs, 10), false)]
fn boot_volume_request_reports_whether_it_is_unset(
    #[case] request: BootVolumeRequest,
    #[case] expected: bool,
) {
    assert_eq!(request.is_unset(), expected);
}

/// Defaulted settings are named for the report.
#[rstest]
#[case::none(CompatibilityDefault { volume_type: false, size: false }, false, "")]
#[case::type_only(
    CompatibilityDefault { volume_type: true, size: false },
    true,
    "type",
)]
#[case::size_only(
    CompatibilityDefault { volume_type: false, size: true },
    true,
    "size",
)]
#[case::both(
    CompatibilityDefault { volume_type: true, size: true },
    true,
    "type, size",
)]
fn compatibility_default_describes_settings(
    #[case] defaulted: CompatibilityDefault,
    #[case] has_default: bool,
    #[case] description: &str,
) {
    assert_eq!(defaulted.has_default(), has_default);
    assert_eq!(defaulted.describe(), description);
}

/// A plan reports its capacity rounded up to whole decimal gigabytes.
#[rstest]
#[case::exact(20_000_000_000, 20)]
#[case::rounded_up(20_000_000_001, 21)]
fn plan_reports_rounded_up_capacity(#[case] size_bytes: u64, #[case] expected_gb: u32) {
    let plan = BootVolumePlan::new(
        BootVolumeType::Local,
        size_bytes,
        CompatibilityDefault::default(),
    );
    assert_eq!(plan.size_gb(), expected_gb);
}

/// A matching root is accepted and recorded.
#[test]
fn allocation_accepts_a_matching_root() {
    let plan = BootVolumePlan::new(
        BootVolumeType::Local,
        20_000_000_000,
        CompatibilityDefault::default(),
    );
    let root = RootVolume {
        id: String::from("vol-root"),
        volume_type: BootVolumeType::Local,
        size_bytes: 20_000_000_000,
    };
    let allocation =
        BootVolumeAllocation::verify("srv-1", &plan, root.clone()).expect("matching root");
    assert_eq!(allocation.root, root);
    assert_eq!(allocation.plan, plan);
}

/// A root that differs in type or size fails, naming both sides.
#[rstest]
#[case::wrong_type(BootVolumeType::Sbs, 20_000_000_000)]
#[case::wrong_size(BootVolumeType::Local, 21_000_000_000)]
fn allocation_rejects_a_differing_root(
    #[case] volume_type: BootVolumeType,
    #[case] size_bytes: u64,
) {
    let plan = BootVolumePlan::new(
        BootVolumeType::Local,
        20_000_000_000,
        CompatibilityDefault::default(),
    );
    let root = RootVolume {
        id: String::from("vol-root"),
        volume_type,
        size_bytes,
    };
    let error = BootVolumeAllocation::verify("srv-1", &plan, root)
        .expect_err("a differing root must be rejected");
    let BootVolumeMismatch::Differs {
        expected, actual, ..
    } = &error
    else {
        panic!("expected a differing-root mismatch, got {error:?}");
    };
    assert!(expected.contains("l_ssd"), "{expected}");
    assert!(expected.contains("20 GB"), "{expected}");
    assert!(actual.contains("GB"), "{actual}");
}
