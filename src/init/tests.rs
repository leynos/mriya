//! Unit tests for init configuration validation and mkfs failure messages.
//!
//! Kills the init validation and mkfs failure-message survivors tracked in
//! #56.

use rstest::rstest;

use super::{InitConfig, InitConfigError, InitRequest, InitRequestError, format_failure_message};
use crate::{
    ScalewayConfig,
    config_store::ConfigStoreError,
    sync::{RemoteCommandOutput, SyncError},
    teardown::append_teardown_note,
};

/// A valid Scaleway configuration that carries `volume_id`, if any.
fn scaleway_with_volume(volume_id: Option<&str>) -> ScalewayConfig {
    ScalewayConfig {
        access_key: None,
        secret_key: String::from("dummy-secret"),
        default_organization_id: None,
        default_project_id: String::from("11111111-2222-3333-4444-555555555555"),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from("DEV1-S"),
        default_image: String::from("Ubuntu 24.04 Noble Numbat"),
        default_architecture: String::from("x86_64"),
        default_volume_id: volume_id.map(str::to_owned),
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    }
}

/// A configured volume ID, including one from the environment, must refuse a
/// replacement unless `--force` was supplied.
#[test]
fn from_config_rejects_a_configured_volume_id_without_force() {
    let scaleway = scaleway_with_volume(Some("env-volume"));
    let init_config = InitConfig { volume_size_gb: 20 };

    let result = InitRequest::from_config(&scaleway, &init_config, "project", false);

    assert!(
        matches!(
            result,
            Err(InitRequestError::Store(
                ConfigStoreError::VolumeAlreadyConfigured { ref volume_id }
            )) if volume_id == "env-volume"
        ),
        "expected VolumeAlreadyConfigured for the configured ID, got {result:?}"
    );
}

/// Forcing the overwrite lets the request through, and the request remembers
/// which ID the resolved configuration supplied so the orchestrator can tell
/// an environment value from a file value.
#[test]
fn from_config_accepts_a_configured_volume_id_with_force() {
    let scaleway = scaleway_with_volume(Some("env-volume"));
    let init_config = InitConfig { volume_size_gb: 20 };

    let request = InitRequest::from_config(&scaleway, &init_config, "project", true)
        .expect("force should permit the overwrite");

    assert_eq!(
        request.configured_volume_id.as_deref(),
        Some("env-volume"),
        "the request should carry the configured ID"
    );
}

/// A blank configured value records nothing, so it never blocks `init`.
#[test]
fn from_config_ignores_a_blank_configured_volume_id() {
    let scaleway = scaleway_with_volume(Some("   "));
    let init_config = InitConfig { volume_size_gb: 20 };

    let request = InitRequest::from_config(&scaleway, &init_config, "project", false)
        .expect("a blank ID should not block init");

    assert_eq!(request.configured_volume_id, None);
}

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
    let failure = super::FormatFailure {
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
    let failure = super::format_spawn_failure(SyncError::HomeUnavailable {
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
    let note = super::append_teardown_note(String::from("provision failed"), Some(&teardown));
    assert_eq!(
        note,
        format!("provision failed (teardown also failed: {teardown})")
    );
}
