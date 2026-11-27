//! Unit tests for init configuration validation and mkfs failure messages.
//!
//! Kills the init validation and mkfs failure-message survivors tracked in
//! #56.

use rstest::rstest;

use super::{FormatFailure, InitConfig, InitConfigError, InitError, format_error};
use crate::{cache_device::parse_report, sync::SyncError, teardown::append_teardown_note};

/// Builds an init configuration with the default tier.
fn config(volume_size_gb: u32) -> InitConfig {
    InitConfig {
        volume_size_gb,
        volume_tier: String::from("sbs_5k"),
    }
}

/// Checks that a volume below the provider minimum returns
/// `InvalidVolumeSize`.
#[test]
fn validate_rejects_a_size_below_the_minimum() {
    let result = config(4).validate();
    assert!(
        matches!(result, Err(InitConfigError::InvalidVolumeSize)),
        "expected InvalidVolumeSize for 4 GB, got {result:?}"
    );
}

/// Checks that the minimum accepted size passes configuration validation.
#[test]
fn validate_accepts_the_minimum_volume_size() {
    let result = config(5).validate();
    assert!(result.is_ok(), "expected Ok for 5 GB size: {result:?}");
}

/// Checks that an unsupported tier is rejected with an actionable error.
#[test]
fn validate_rejects_an_unknown_volume_tier() {
    let mut init = config(20);
    init.volume_tier = String::from("b_ssd");
    let result = init.validate();
    assert!(
        matches!(result, Err(InitConfigError::InvalidVolumeTier { ref value }) if value == "b_ssd"),
        "expected InvalidVolumeTier, got {result:?}"
    );
}

/// Checks that both supported tiers parse.
#[test]
fn validate_accepts_both_supported_tiers() {
    for tier in ["sbs_5k", "sbs_15k"] {
        let mut init = config(20);
        init.volume_tier = String::from(tier);
        assert!(init.validate().is_ok(), "expected {tier} to pass");
        assert_eq!(init.tier().expect("tier parses").as_str(), tier);
    }
}

/// The exact typed error a cache-device refusal must produce.
#[derive(Clone, Copy, Debug)]
enum Refusal {
    Signature,
    RootDisk,
    NotFound,
    Ambiguous,
    RootUnknown,
    Failed,
}

impl Refusal {
    /// Reports whether `error` is this refusal's variant.
    fn matches(self, error: &InitError<std::io::Error>) -> bool {
        match self {
            Self::Signature => matches!(error, InitError::FormatSignaturePresent { .. }),
            Self::RootDisk => matches!(error, InitError::FormatRootDisk { .. }),
            Self::NotFound => matches!(error, InitError::FormatDeviceNotFound { .. }),
            Self::Ambiguous => matches!(error, InitError::FormatDeviceAmbiguous { .. }),
            Self::RootUnknown => matches!(error, InitError::FormatRootUnknown { .. }),
            Self::Failed => matches!(error, InitError::FormatFailed { .. }),
        }
    }
}

/// Covers each cache-device refusal and the exact error it produces.
#[rstest]
#[case(
    "signature-present DEVICE=/dev/sdb FSTYPE=ext4",
    Refusal::Signature,
    "signature"
)]
#[case("root-disk DEVICE=/dev/sda", Refusal::RootDisk, "root")]
#[case("not-found DEVICE=serial:volume-1", Refusal::NotFound, "not-found")]
#[case("ambiguous DEVICE=serial:volume-1", Refusal::Ambiguous, "ambiguous")]
#[case("root-unknown DEVICE=/dev/sdb", Refusal::RootUnknown, "root")]
#[case("format-failed DEVICE=/dev/sdb", Refusal::Failed, "format")]
fn format_refusals_map_to_typed_errors(
    #[case] line: &str,
    #[case] expected: Refusal,
    #[case] marker: &str,
) {
    let report = parse_report(&format!("MRIYA_CACHE_STATUS={line}\n")).expect("status parses");
    let error = format_error::<std::io::Error>("volume-1", FormatFailure::Report(report));
    let rendered = error.to_string();
    assert!(
        rendered.contains(marker),
        "expected `{marker}` in {rendered}"
    );
    assert!(rendered.contains("volume-1"), "{rendered}");
    assert!(
        expected.matches(&error),
        "expected {expected:?}, got {error:?}"
    );
}

/// Proves the `init` diagnostic keeps the cause and the remedy.
///
/// `write_error` prints only the outermost `Display` and no cause chain, so a
/// message that stored the `SyncError` without rendering it would report a bare
/// format failure. The three fragments are what a user needs to stop looking
/// for a broken volume and fix `HOME` instead.
#[test]
fn format_failure_names_the_unusable_home() {
    let failure = super::format_spawn_failure(SyncError::HomeUnavailable {
        path: String::from("~/keys/id"),
        source: crate::sync::HomeUnavailable,
    });

    let FormatFailure::Spawn { message, .. } = failure else {
        panic!("expected a spawn failure");
    };
    for expected in [
        "~/keys/id",
        "HOME is set to a value that is not valid UTF-8",
        "configure an absolute ssh_identity_file",
    ] {
        assert!(
            message.contains(expected),
            "`{expected}` should appear in the diagnostic: {message}"
        );
    }
}

/// Ensures successful teardown leaves the original provisioning error intact.
#[test]
fn append_teardown_note_returns_message_unchanged_without_error() {
    let note = append_teardown_note::<SyncError>(String::from("provision failed"), None);
    assert_eq!(note, "provision failed");
}

/// Checks that teardown failure is appended without replacing the primary
/// error.
#[test]
fn append_teardown_note_appends_teardown_failure() {
    let teardown = SyncError::InvalidConfig {
        field: String::from("ssh_user"),
    };
    let note = append_teardown_note(String::from("provision failed"), Some(&teardown));
    assert_eq!(
        note,
        format!("provision failed (teardown also failed: {teardown})")
    );
}
