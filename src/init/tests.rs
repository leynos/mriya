//! Unit tests for init configuration validation and mkfs failure messages.
//!
//! Kills the init validation and mkfs failure-message survivors tracked in
//! #56.

use rstest::rstest;

use super::{InitConfig, InitConfigError, format_failure_message};
use crate::sync::{RemoteCommandOutput, SyncError};

#[test]
fn validate_rejects_zero_volume_size() {
    let config = InitConfig { volume_size_gb: 0 };
    let result = config.validate();
    assert!(
        matches!(result, Err(InitConfigError::InvalidVolumeSize)),
        "expected InvalidVolumeSize for zero size, got {result:?}"
    );
}

#[test]
fn validate_accepts_non_zero_volume_size() {
    let config = InitConfig { volume_size_gb: 20 };
    let result = config.validate();
    assert!(result.is_ok(), "expected Ok for non-zero size: {result:?}");
}

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

#[test]
fn format_failure_display_writes_the_message() {
    let failure = super::FormatFailure {
        message: String::from("mkfs.ext4 exited with status 1"),
        source: None,
    };
    assert_eq!(failure.to_string(), "mkfs.ext4 exited with status 1");
}

#[test]
fn append_teardown_note_returns_message_unchanged_without_error() {
    let note = super::append_teardown_note::<SyncError>(String::from("provision failed"), None);
    assert_eq!(note, "provision failed");
}

#[test]
fn append_teardown_note_appends_teardown_failure() {
    let teardown = SyncError::InvalidConfig {
        field: String::from("ssh_user"),
    };
    let note = super::append_teardown_note(String::from("provision failed"), Some(&teardown));
    assert_eq!(
        note,
        format!("provision failed (teardown also failed: {teardown})")
    );
}
