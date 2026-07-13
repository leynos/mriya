//! Unit tests for init configuration validation and mkfs failure messages.

use rstest::rstest;

use super::{InitConfig, InitConfigError, format_failure_message};
use crate::sync::RemoteCommandOutput;

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
