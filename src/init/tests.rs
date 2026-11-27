//! Unit tests for init configuration validation and mkfs failure messages.
//!
//! Kills the init validation and mkfs failure-message survivors tracked in
//! #56.

use std::sync::Arc;

use camino::Utf8Path;
use ortho_config::{MapEnv, OrthoConfig as _};
use rstest::rstest;

use super::{
    InitConfig,
    InitConfigError,
    InitRequest,
    InitRequestError,
    InitRequestInputs,
    VOLUME_ID_ENV_VAR,
    environment_volume_id,
    format_failure_message,
    request_from_loaded_config,
};
use crate::{
    ScalewayConfig,
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

/// A file-backed volume ID must refuse a replacement unless `--force` was
/// supplied; the error tells the user to rerun with the flag.
#[test]
fn request_builder_rejects_a_file_volume_id_without_force() {
    let scaleway = scaleway_with_volume(Some("file-volume"));
    let init_config = InitConfig { volume_size_gb: 20 };
    let env = MapEnv::new();

    let err = request_from_loaded_config(
        InitRequestInputs {
            scaleway: &scaleway,
            init_config: &init_config,
            project_name: "project",
            overwrite_existing_volume_id: false,
        },
        &env,
    )
    .expect_err("a file-backed ID must be refused without force");

    assert!(
        matches!(
            err,
            InitRequestError::FileVolumeAlreadyConfigured { ref volume_id }
                if volume_id == "file-volume"
        ),
        "expected a file-backed refusal, got {err}"
    );
    assert!(
        err.to_string().contains("--force"),
        "a file-backed refusal should mention --force: {err}"
    );
}

/// Forcing the overwrite lets a file-backed ID through, and the request
/// records it separately from the environment provenance.
#[test]
fn request_builder_accepts_a_file_volume_id_with_force() {
    let scaleway = scaleway_with_volume(Some("file-volume"));
    let init_config = InitConfig { volume_size_gb: 20 };
    let env = MapEnv::new();

    let request = request_from_loaded_config(
        InitRequestInputs {
            scaleway: &scaleway,
            init_config: &init_config,
            project_name: "project",
            overwrite_existing_volume_id: true,
        },
        &env,
    )
    .expect("force should permit the file-backed overwrite");

    assert_eq!(
        request.configured_volume_id.as_deref(),
        Some("file-volume"),
        "the request should carry the configured ID"
    );
    assert_eq!(request.environment_volume_id, None);
}

/// A blank resolved value records nothing here, so request construction does
/// not block; a blank environment value is caught separately by the
/// orchestrator's provenance guard.
#[test]
fn from_config_ignores_a_blank_configured_volume_id() {
    let scaleway = scaleway_with_volume(Some("   "));
    let init_config = InitConfig { volume_size_gb: 20 };

    let request = InitRequest::from_config(&scaleway, &init_config, "project", false)
        .expect("a blank ID should not block init");

    assert_eq!(request.configured_volume_id, None);
}

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

/// A scanning environment map with `vars` set.
fn volume_env(vars: &[(&str, &std::ffi::OsStr)]) -> MapEnv {
    vars.iter()
        .fold(MapEnv::new(), |env, (key, value)| env.with_var(*key, value))
}

#[test]
fn environment_volume_id_is_absent_when_unset() {
    let env = volume_env(&[]);

    let result = environment_volume_id(&env).expect("an unset variable should be accepted");

    assert_eq!(result, None);
}

#[test]
fn environment_volume_id_is_trimmed_when_set() {
    let env = volume_env(&[(VOLUME_ID_ENV_VAR, std::ffi::OsStr::new("  volume-123 "))]);

    let result = environment_volume_id(&env).expect("a set variable should be accepted");

    assert_eq!(result.as_deref(), Some("volume-123"));
}

/// The loader matches environment keys case-insensitively, so the guard must
/// too.
#[test]
fn environment_volume_id_matches_a_case_variant_key() {
    let env = volume_env(&[("scw_default_volume_id", std::ffi::OsStr::new("old-id"))]);

    let result = environment_volume_id(&env).expect("a case variant should be accepted");

    assert_eq!(result.as_deref(), Some("old-id"));
}

#[test]
fn environment_volume_id_records_a_blank_value_for_the_orchestrator() {
    let env = volume_env(&[(VOLUME_ID_ENV_VAR, std::ffi::OsStr::new("   "))]);

    let result = environment_volume_id(&env).expect("a blank value is recorded, not rejected");

    assert_eq!(
        result.as_deref(),
        Some(""),
        "the blank layer should be recorded so the guard sees it"
    );
}

#[cfg(unix)]
#[test]
fn environment_volume_id_rejects_a_non_utf8_value() {
    use std::os::unix::ffi::OsStrExt as _;

    let env = volume_env(&[(VOLUME_ID_ENV_VAR, std::ffi::OsStr::from_bytes(&[0xff]))]);

    let err = environment_volume_id(&env).expect_err("a non-UTF-8 variable must be rejected");

    assert!(
        err.to_string().contains("not valid UTF-8"),
        "unexpected error: {err}"
    );
}

/// The loader keeps a blank environment layer over the file, which is why the
/// provenance records it as empty and the orchestrator refuses it.
#[test]
fn a_blank_environment_volume_id_survives_loading_and_is_recorded() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root = Utf8Path::from_path(tmp.path()).expect("temp path should be UTF-8");
    let file = root.join("mriya.toml");
    cap_std::fs_utf8::Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .expect("temp directory should open")
        .write(
            "mriya.toml",
            concat!(
                "secret_key = \"secret\"\n",
                "default_project_id = \"project\"\n",
                "default_volume_id = \"file-volume\"\n"
            ),
        )
        .expect("config fixture should be written");
    let env = MapEnv::new()
        .with_current_dir(root)
        .with_var("MRIYA_CONFIG_PATH", file.as_str())
        .with_var(VOLUME_ID_ENV_VAR, "   ");
    let shared = Arc::new(env);
    let config =
        ScalewayConfig::load_from_iter_with_sources(["mriya"], shared.clone(), shared.clone())
            .expect("configuration should load");

    assert!(
        config
            .default_volume_id
            .as_deref()
            .is_some_and(|volume_id| volume_id.trim().is_empty()),
        "the loader should retain a blank layer over the file, got {:?}",
        config.default_volume_id
    );

    let recorded = environment_volume_id(shared.as_ref())
        .expect("the blank layer should be recorded, not rejected");

    assert_eq!(
        recorded.as_deref(),
        Some(""),
        "the provenance should record the blank environment layer"
    );
}

proptest::proptest! {
    /// Any casing of the canonical spelling is found, and the value is trimmed.
    #[test]
    fn environment_volume_id_matches_any_key_casing(
        bits in proptest::collection::vec(
            proptest::prelude::any::<bool>(),
            VOLUME_ID_ENV_VAR.len()..=VOLUME_ID_ENV_VAR.len(),
        ),
        id in "[A-Za-z0-9_-]{1,24}",
        leading in "[ \\t]{0,4}",
        trailing in "[ \\t]{0,4}",
    ) {
        let key: String = VOLUME_ID_ENV_VAR
            .chars()
            .zip(bits.iter())
            .map(|(character, uppercase)| {
                if *uppercase {
                    character.to_ascii_uppercase()
                } else {
                    character.to_ascii_lowercase()
                }
            })
            .collect();
        let value = format!("{leading}{id}{trailing}");
        let env = MapEnv::new().with_var(key.as_str(), value.as_str());

        let found = environment_volume_id(&env).expect("a UTF-8 value should be accepted");

        assert_eq!(found.as_deref(), Some(id.as_str()));
    }
}
