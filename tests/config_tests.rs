//! Unit tests for configuration and request validation.

#[path = "common/test_constants.rs"]
mod test_constants;

use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs_utf8::Dir};
use mockable::MockEnv;
use mriya::{BootVolumeType, ScalewayConfig, config::ConfigError};
use rstest::*;
use tempfile::TempDir;
use test_constants::DEFAULT_INSTANCE_TYPE;

/// An environment whose `HOME` answers `home`, so `as_request_with_env` never
/// reads the process environment. A lookup of any other key fails the test, so
/// an accidental extra read cannot pass unnoticed.
fn env_with_home(home: Result<String, std::env::VarError>) -> MockEnv {
    let for_string = home.clone();
    let mut env = MockEnv::new();
    env.expect_raw().returning(move |key| {
        assert_eq!(key, "HOME", "only HOME may be read");
        home.clone()
    });
    env.expect_string().returning(move |key| {
        assert_eq!(key, "HOME", "only HOME may be read");
        for_string.clone().ok()
    });
    env
}

#[fixture]
fn valid_config() -> ScalewayConfig {
    ScalewayConfig {
        access_key: Some(String::from("SCWACCESSKEYEXAMPLE")),
        secret_key: String::from("SCWSECRETKEYEXAMPLE"),
        default_organization_id: None,
        default_project_id: String::from("11111111-2222-3333-4444-555555555555"),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from(DEFAULT_INSTANCE_TYPE),
        default_image: String::from("ubuntu-22-04"),
        default_architecture: String::from("x86_64"),
        default_volume_id: None,
        boot_volume_type: None,
        boot_volume_size_gb: None,
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    }
}

/// Helper to create a temporary cloud-init user-data file for testing.
/// Returns the `TempDir` (must be kept alive) and the file path as a String.
fn write_temp_cloud_init_file(filename: &str, content: &str) -> anyhow::Result<(TempDir, String)> {
    use anyhow::Context as _;
    let tmp = TempDir::new().context("tempdir")?;
    let path = tmp.path().join(filename);
    let tmp_root =
        Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).map_err(|non_utf8_path| {
            anyhow::anyhow!("temp dir should be utf8: {}", non_utf8_path.display())
        })?;
    Dir::open_ambient_dir(&tmp_root, ambient_authority())
        .context("open temp dir")?
        .write(filename, content)
        .context("write file")?;
    let path_str = path
        .to_str()
        .with_context(|| format!("temp path should be utf8: {}", path.display()))?
        .to_owned();
    Ok((tmp, path_str))
}

/// Verifies that validation produces actionable errors mentioning both the
/// environment variable and configuration file for each required field.
#[rstest]
#[case::secret_key(
    |cfg: &mut ScalewayConfig| cfg.secret_key.clear(),
    "SCW_SECRET_KEY",
    "secret_key"
)]
#[case::project_id(
    |cfg: &mut ScalewayConfig| cfg.default_project_id.clear(),
    "SCW_DEFAULT_PROJECT_ID",
    "default_project_id"
)]
#[case::image(
    |cfg: &mut ScalewayConfig| cfg.default_image.clear(),
    "SCW_DEFAULT_IMAGE",
    "default_image"
)]
#[case::instance_type(
    |cfg: &mut ScalewayConfig| cfg.default_instance_type.clear(),
    "SCW_DEFAULT_INSTANCE_TYPE",
    "default_instance_type"
)]
#[case::zone(
    |cfg: &mut ScalewayConfig| cfg.default_zone.clear(),
    "SCW_DEFAULT_ZONE",
    "default_zone"
)]
#[case::architecture(
    |cfg: &mut ScalewayConfig| cfg.default_architecture.clear(),
    "SCW_DEFAULT_ARCHITECTURE",
    "default_architecture"
)]
fn config_validation_produces_actionable_errors(
    valid_config: ScalewayConfig,
    #[case] clear_field: fn(&mut ScalewayConfig),
    #[case] env_var: &str,
    #[case] toml_key: &str,
) {
    let mut cfg = valid_config;
    clear_field(&mut cfg);
    let error = cfg.validate().expect_err("validation should fail");
    let ConfigError::MissingField(ref message) = error else {
        panic!("expected MissingField error, got {error:?}");
    };
    assert!(
        message.contains(env_var),
        "error should mention env var {env_var}: {message}"
    );
    assert!(
        message.contains("mriya.toml"),
        "error should mention config file: {message}"
    );
    assert!(
        message.contains(toml_key),
        "error should mention TOML key {toml_key}: {message}"
    );
}

#[rstest]
fn config_as_request_produces_valid_request(valid_config: ScalewayConfig) {
    let cfg = valid_config;
    let request = cfg
        .as_request()
        .expect("valid configuration should produce an instance request");
    request
        .validate()
        .expect("request produced from valid configuration should validate");
    assert_eq!(request.image_label, cfg.default_image);
    assert_eq!(request.instance_type, cfg.default_instance_type);
    assert_eq!(request.zone, cfg.default_zone);
    assert_eq!(request.project_id, cfg.default_project_id);
    assert_eq!(request.architecture, cfg.default_architecture);
    assert_eq!(request.volume_id, cfg.default_volume_id);
    assert_eq!(request.cloud_init_user_data, None);
}

#[rstest]
#[case::inline_and_file_together(
    |cfg: &mut ScalewayConfig| {
        cfg.cloud_init_user_data = Some(String::from("#cloud-config\npackages: [jq]\n"));
        cfg.cloud_init_user_data_file = Some(String::from("/tmp/user-data.yml"));
    },
    "SCW_CLOUD_INIT_USER_DATA"
)]
#[case::empty_inline(
    |cfg: &mut ScalewayConfig| cfg.cloud_init_user_data = Some(String::from("   ")),
    "cloud-init user-data must not be empty"
)]
fn config_rejects_invalid_inline_cloud_init(
    valid_config: ScalewayConfig,
    #[case] apply_invalid: fn(&mut ScalewayConfig),
    #[case] expected_message: &str,
) {
    let mut cfg = valid_config;
    apply_invalid(&mut cfg);

    let err = cfg
        .as_request()
        .expect_err("invalid inline cloud-init should be rejected")
        .to_string();
    assert!(err.contains(expected_message), "unexpected error: {err}");
}

#[rstest]
fn config_reads_cloud_init_user_data_from_file(valid_config: ScalewayConfig) {
    let (_tmp, path_str) = write_temp_cloud_init_file("user-data.txt", "file-user-data")
        .expect("cloud-init fixture file should be created");

    let cfg = ScalewayConfig {
        cloud_init_user_data_file: Some(path_str),
        ..valid_config
    };

    let request = cfg
        .as_request()
        .expect("configuration with a cloud-init fixture should produce a request");
    assert_eq!(
        request.cloud_init_user_data,
        Some(String::from("file-user-data"))
    );
}

#[rstest]
fn config_errors_when_cloud_init_user_data_file_missing(valid_config: ScalewayConfig) {
    let tmp = TempDir::new()
        .expect("temporary directory for the missing cloud-init fixture should be created");
    let missing_path = tmp.path().join("does-not-exist.txt");
    let missing_path_str = missing_path
        .to_str()
        .expect("temporary missing-file path should be valid UTF-8")
        .to_owned();

    let cfg = ScalewayConfig {
        cloud_init_user_data_file: Some(missing_path_str.clone()),
        ..valid_config
    };

    let err = cfg
        .as_request()
        .expect_err("expected missing user-data file to error");

    let ConfigError::CloudInitFileRead { path, .. } = err else {
        panic!("expected CloudInitFileRead error");
    };

    assert_eq!(path, missing_path_str, "expected error path to match");
}

#[rstest]
fn config_errors_when_cloud_init_user_data_file_is_empty(valid_config: ScalewayConfig) {
    let (_tmp, path_str) = write_temp_cloud_init_file("user-data-empty.txt", "   \n\t  ")
        .expect("empty cloud-init fixture file should be created");

    let cfg = ScalewayConfig {
        cloud_init_user_data_file: Some(path_str),
        ..valid_config
    };

    let err = cfg
        .as_request()
        .expect_err("expected whitespace-only user-data file to error");

    let ConfigError::CloudInit(message) = err else {
        panic!("expected CloudInit error");
    };

    assert!(
        message.contains("cloud-init user-data file must not be empty"),
        "unexpected error: {message}"
    );
}

/// Proves a non-UTF-8 `HOME` stops the request before any file is read.
///
/// This is the public entry point a real run takes, so it is the closest
/// end-to-end evidence available without cloud access: the failure happens in
/// `as_request`, which `run_command` calls before provisioning or syncing, so
/// the remote command is never reached. The message must name both the path
/// and the remedy, because `write_error` prints only the outermost `Display`.
#[rstest]
fn config_rejects_a_tilde_cloud_init_file_when_home_cannot_be_decoded(
    mut valid_config: ScalewayConfig,
) {
    use std::ffi::OsString;

    valid_config.cloud_init_user_data_file = Some(String::from("~/user-data.txt"));

    let env = env_with_home(Err(std::env::VarError::NotUnicode(OsString::from(
        "/home/\u{fffd}",
    ))));
    let err = valid_config
        .as_request_with_env(&env)
        .expect_err("an undecodable HOME should be reported, not silently ignored");

    let rendered = err.to_string();
    for expected in [
        "~/user-data.txt",
        "HOME is set to a value that is not valid UTF-8",
        "absolute",
    ] {
        assert!(
            rendered.contains(expected),
            "`{expected}` should appear in the diagnostic: {rendered}"
        );
    }
}

#[rstest]
fn config_expands_tilde_for_cloud_init_user_data_file(valid_config: ScalewayConfig) {
    let tmp = TempDir::new().expect("temporary home directory should be created");
    let home = tmp.path().to_string_lossy().to_string();

    let tmp_root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf())
        .expect("temporary home directory path should be valid UTF-8");
    let fs = Dir::open_ambient_dir(&tmp_root, ambient_authority())
        .expect("temporary home directory should open");
    fs.create_dir_all("cloud-init")
        .expect("cloud-init fixture directory should be created");
    fs.write("cloud-init/user-data.txt", "tilde-user-data")
        .expect("tilde-expanded cloud-init fixture should be written");

    let cfg = ScalewayConfig {
        cloud_init_user_data_file: Some(String::from("~/cloud-init/user-data.txt")),
        ..valid_config
    };

    // `as_request_with_env` reads `HOME` from the injected environment.
    let env = env_with_home(Ok(home.clone()));
    let request = cfg
        .as_request_with_env(&env)
        .expect("configuration with a tilde-expanded cloud-init file should produce a request");

    assert_eq!(
        request.cloud_init_user_data,
        Some(String::from("tilde-user-data"))
    );
}

/// Boot-volume settings reach the request unchanged, keeping the cache volume
/// separate.
///
/// The cache volume ID and the boot-storage settings travel on different
/// fields: a request that sets both must show both, and a request that sets
/// only one must leave the other at its default.
#[rstest]
#[case::both_explicit(Some("l_ssd"), Some(20), Some(BootVolumeType::Local), Some(20))]
#[case::type_only(Some("sbs_volume"), None, Some(BootVolumeType::Sbs), None)]
#[case::size_only(None, Some(30), None, Some(30))]
#[case::neither(None, None, None, None)]
fn boot_volume_settings_reach_the_request(
    valid_config: ScalewayConfig,
    #[case] configured_type: Option<&str>,
    #[case] configured_size: Option<u32>,
    #[case] expected_type: Option<BootVolumeType>,
    #[case] expected_size: Option<u32>,
) {
    let cfg = ScalewayConfig {
        default_volume_id: Some(String::from("vol-cache")),
        boot_volume_type: configured_type.map(str::to_owned),
        boot_volume_size_gb: configured_size,
        ..valid_config
    };

    let request = cfg
        .as_request_with_env(&env_with_home(Err(std::env::VarError::NotPresent)))
        .expect("boot-volume settings should produce a request");

    assert_eq!(request.boot_volume.volume_type, expected_type);
    assert_eq!(request.boot_volume.size_gb, expected_size);
    assert_eq!(
        request.volume_id.as_deref(),
        Some("vol-cache"),
        "the cache volume must be unaffected by the boot settings"
    );
}

/// A boot-volume type the provider no longer accepts is refused by name,
/// naming both routes a user can change.
#[rstest]
fn boot_volume_type_rejects_unsupported_identifiers(valid_config: ScalewayConfig) {
    let cfg = ScalewayConfig {
        boot_volume_type: Some(String::from("b_ssd")),
        ..valid_config
    };

    let error = cfg
        .resolve_boot_volume()
        .expect_err("a deprecated volume type must be refused");
    let message = error.to_string();
    assert!(message.contains("b_ssd"), "{message}");
    assert!(message.contains("SCW_BOOT_VOLUME_TYPE"), "{message}");
    assert!(message.contains("boot_volume_type"), "{message}");
}

/// A blank boot-volume type is treated as unset rather than refused, so an
/// empty environment variable cannot break a run.
#[rstest]
#[case::blank("   ")]
#[case::empty("")]
fn blank_boot_volume_type_is_unset(valid_config: ScalewayConfig, #[case] value: &str) {
    let cfg = ScalewayConfig {
        boot_volume_type: Some(value.to_owned()),
        ..valid_config
    };

    let resolved = cfg
        .resolve_boot_volume()
        .expect("a blank type should be treated as unset");
    assert_eq!(resolved.volume_type, None);
}

/// A zero boot-volume size is refused, naming the variable and the key.
#[rstest]
fn boot_volume_size_rejects_zero(valid_config: ScalewayConfig) {
    let cfg = ScalewayConfig {
        boot_volume_size_gb: Some(0),
        ..valid_config
    };

    let error = cfg
        .resolve_boot_volume()
        .expect_err("a zero size must be refused");
    let message = error.to_string();
    assert!(message.contains("SCW_BOOT_VOLUME_SIZE_GB"), "{message}");
    assert!(message.contains("boot_volume_size_gb"), "{message}");
}
