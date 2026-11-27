//! Unit tests for init configuration validation and mkfs failure messages.
//!
//! Kills the init validation and mkfs failure-message survivors tracked in
//! #56.

use rstest::rstest;

use super::{
    InitConfig,
    InitConfigError,
    format::{FormatFailure, format_failure_message, format_spawn_failure},
};
use crate::sync::{RemoteCommandOutput, SyncError};

/// Checks that zero-sized volumes return `InvalidVolumeSize`.
#[test]
fn validate_rejects_zero_volume_size() {
    let config = InitConfig { volume_size_gb: 0 };
    let result = config.validate();
    assert!(
        matches!(result, Err(InitConfigError::InvalidVolumeSize)),
        "expected InvalidVolumeSize for zero size, got {result:?}"
    );
}

/// Checks that a non-zero volume size passes configuration validation.
#[test]
fn validate_accepts_non_zero_volume_size() {
    let config = InitConfig { volume_size_gb: 20 };
    let result = config.validate();
    assert!(result.is_ok(), "expected Ok for non-zero size: {result:?}");
}

/// Covers present and missing `mkfs` exit statuses, with and without stderr.
#[rstest]
#[case(Some(1), "", "mkfs.ext4 exited with status 1")]
#[case(Some(1), "boom\n", "mkfs.ext4 exited with status 1: boom")]
#[case(None, "", "mkfs.ext4 terminated without an exit status")]
#[case(None, "boom\n", "mkfs.ext4 terminated without an exit status: boom")]
fn format_failure_message_covers_every_arm(
    #[case] exit_code: Option<i32>,
    #[case] stderr: &str,
    #[case] expected: &str,
) {
    let output = RemoteCommandOutput {
        exit_code,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    };
    assert_eq!(format_failure_message(&output), expected);
}

/// Verifies the public display text for a formatting failure.
#[test]
fn format_failure_display_writes_the_message() {
    let failure = FormatFailure {
        message: String::from("mkfs.ext4 exited with status 1"),
        source: None,
    };
    assert_eq!(failure.to_string(), "mkfs.ext4 exited with status 1");
}

/// Proves the `init` diagnostic keeps the cause and the remedy.
///
/// `write_error` prints only the outermost `Display` and no cause chain, so a
/// message that stored the `SyncError` without rendering it would report a bare
/// format failure. The three fragments are what a user needs to stop looking
/// for a broken volume and fix `HOME` instead.
#[test]
fn format_failure_names_the_unusable_home() {
    let failure = format_spawn_failure(SyncError::HomeUnavailable {
        path: String::from("~/keys/id"),
        source: crate::sync::HomeUnavailable,
    });

    let rendered = failure.to_string();
    for expected in [
        "~/keys/id",
        "HOME is set to a value that is not valid UTF-8",
        "configure an absolute ssh_identity_file",
    ] {
        assert!(
            rendered.contains(expected),
            "`{expected}` should appear in the diagnostic: {rendered}"
        );
    }
}
