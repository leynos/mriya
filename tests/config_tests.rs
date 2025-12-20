//! Unit tests for configuration and request validation.

#[path = "common/test_constants.rs"]
mod test_constants;

use mriya::{ScalewayConfig, config::ConfigError};
use rstest::*;
use tempfile::TempDir;

use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs_utf8::Dir};

use test_constants::DEFAULT_INSTANCE_TYPE;

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
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    }
}

#[test]
fn config_validation_rejects_missing_secret_with_actionable_error() {
    let cfg = ScalewayConfig {
        secret_key: String::new(),
        ..valid_config()
    };

    let error = cfg.validate().expect_err("secret is required");
    let ConfigError::MissingField(ref message) = error else {
        panic!("expected MissingField error");
    };
    assert!(
        message.contains("SCW_SECRET_KEY"),
        "error should mention env var: {message}"
    );
    assert!(
        message.contains("mriya.toml"),
        "error should mention config file: {message}"
    );
    assert!(
        message.contains("secret_key"),
        "error should mention TOML key: {message}"
    );
}

/// Verifies that validation produces actionable errors mentioning both the
/// environment variable and configuration file for each required field.
#[test]
fn config_validation_produces_actionable_errors_for_all_fields() {
    fn assert_actionable(
        mut cfg: ScalewayConfig,
        mutate: impl FnOnce(&mut ScalewayConfig),
        env_var: &str,
        toml_key: &str,
    ) {
        mutate(&mut cfg);
        let error = cfg.validate().expect_err("validation should fail");
        let message = error.to_string();
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

    assert_actionable(
        valid_config(),
        |cfg| cfg.default_project_id.clear(),
        "SCW_DEFAULT_PROJECT_ID",
        "default_project_id",
    );

    assert_actionable(
        valid_config(),
        |cfg| cfg.default_image.clear(),
        "SCW_DEFAULT_IMAGE",
        "default_image",
    );

    assert_actionable(
        valid_config(),
        |cfg| cfg.default_instance_type.clear(),
        "SCW_DEFAULT_INSTANCE_TYPE",
        "default_instance_type",
    );

    assert_actionable(
        valid_config(),
        |cfg| cfg.default_zone.clear(),
        "SCW_DEFAULT_ZONE",
        "default_zone",
    );

    assert_actionable(
        valid_config(),
        |cfg| cfg.default_architecture.clear(),
        "SCW_DEFAULT_ARCHITECTURE",
        "default_architecture",
    );
}

#[test]
fn config_as_request_produces_valid_request() {
    let cfg = valid_config();
    let request = cfg
        .as_request()
        .unwrap_or_else(|err| panic!("valid config yields request: {err}"));
    request
        .validate()
        .unwrap_or_else(|err| panic!("request from config validates: {err}"));
    assert_eq!(request.image_label, cfg.default_image);
    assert_eq!(request.instance_type, cfg.default_instance_type);
    assert_eq!(request.zone, cfg.default_zone);
    assert_eq!(request.project_id, cfg.default_project_id);
    assert_eq!(request.architecture, cfg.default_architecture);
    assert_eq!(request.volume_id, cfg.default_volume_id);
    assert_eq!(request.cloud_init_user_data, None);
}

#[test]
fn config_rejects_cloud_init_inline_and_file_together() {
    let cfg = ScalewayConfig {
        cloud_init_user_data: Some(String::from("#cloud-config\npackages: [jq]\n")),
        cloud_init_user_data_file: Some(String::from("/tmp/user-data.yml")),
        ..valid_config()
    };

    let err = cfg.validate().expect_err("expected conflict to error");
    assert!(
        err.to_string().contains("SCW_CLOUD_INIT_USER_DATA"),
        "unexpected error: {err}"
    );
}

#[test]
fn config_rejects_empty_cloud_init_inline() {
    let cfg = ScalewayConfig {
        cloud_init_user_data: Some(String::from("   ")),
        ..valid_config()
    };

    let err = cfg.validate().expect_err("expected empty inline to error");
    assert!(
        err.to_string()
            .contains("cloud-init user-data must not be empty"),
        "unexpected error: {err}"
    );
}

#[test]
fn config_reads_cloud_init_user_data_from_file() {
    let tmp = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
    let path = tmp.path().join("user-data.txt");
    let tmp_root =
        Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap_or_else(|non_utf8_path| {
            panic!("temp dir should be utf8: {}", non_utf8_path.display())
        });
    Dir::open_ambient_dir(&tmp_root, ambient_authority())
        .unwrap_or_else(|err| panic!("open temp dir: {err}"))
        .write("user-data.txt", "file-user-data")
        .unwrap_or_else(|err| panic!("write file: {err}"));
    let path_str = path
        .to_str()
        .unwrap_or_else(|| panic!("temp path should be utf8: {}", path.display()))
        .to_owned();

    let cfg = ScalewayConfig {
        cloud_init_user_data_file: Some(path_str),
        ..valid_config()
    };

    let request = cfg
        .as_request()
        .unwrap_or_else(|err| panic!("as_request should succeed: {err}"));
    assert_eq!(
        request.cloud_init_user_data,
        Some(String::from("file-user-data"))
    );
}

#[test]
fn config_errors_when_cloud_init_user_data_file_missing() {
    let tmp = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
    let missing_path = tmp.path().join("does-not-exist.txt");
    let missing_path_str = missing_path
        .to_str()
        .unwrap_or_else(|| panic!("temp path should be utf8: {}", missing_path.display()))
        .to_owned();

    let cfg = ScalewayConfig {
        cloud_init_user_data_file: Some(missing_path_str.clone()),
        ..valid_config()
    };

    let err = cfg
        .validate()
        .expect_err("expected missing user-data file to error");

    let ConfigError::CloudInitFileRead { path, .. } = err else {
        panic!("expected CloudInitFileRead error");
    };

    assert_eq!(path, missing_path_str, "expected error path to match");
}

#[test]
fn config_errors_when_cloud_init_user_data_file_is_empty() {
    let tmp = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
    let path = tmp.path().join("user-data-empty.txt");
    let tmp_root =
        Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap_or_else(|non_utf8_path| {
            panic!("temp dir should be utf8: {}", non_utf8_path.display())
        });
    Dir::open_ambient_dir(&tmp_root, ambient_authority())
        .unwrap_or_else(|err| panic!("open temp dir: {err}"))
        .write("user-data-empty.txt", "   \n\t  ")
        .unwrap_or_else(|err| panic!("write empty file: {err}"));
    let path_str = path
        .to_str()
        .unwrap_or_else(|| panic!("temp path should be utf8: {}", path.display()))
        .to_owned();

    let cfg = ScalewayConfig {
        cloud_init_user_data_file: Some(path_str),
        ..valid_config()
    };

    let err = cfg
        .validate()
        .expect_err("expected whitespace-only user-data file to error");

    let ConfigError::CloudInit(message) = err else {
        panic!("expected CloudInit error");
    };

    assert!(
        message.contains("cloud-init user-data file must not be empty"),
        "unexpected error: {message}"
    );
}

#[tokio::test]
async fn config_expands_tilde_for_cloud_init_user_data_file() {
    let tmp = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
    let home = tmp.path().to_string_lossy().to_string();
    let _guard = mriya::test_support::EnvGuard::set_vars(&[("HOME", home.as_str())]).await;

    let tmp_root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf())
        .unwrap_or_else(|path| panic!("temp home dir should be utf8: {}", path.display()));
    let fs = Dir::open_ambient_dir(&tmp_root, ambient_authority())
        .unwrap_or_else(|err| panic!("open temp home dir: {err}"));
    fs.create_dir_all("cloud-init")
        .unwrap_or_else(|err| panic!("create cloud-init dir: {err}"));
    fs.write("cloud-init/user-data.txt", "tilde-user-data")
        .unwrap_or_else(|err| panic!("write tilde user-data file: {err}"));

    let cfg = ScalewayConfig {
        cloud_init_user_data_file: Some(String::from("~/cloud-init/user-data.txt")),
        ..valid_config()
    };

    let request = cfg
        .as_request()
        .unwrap_or_else(|err| panic!("as_request should succeed: {err}"));

    assert_eq!(
        request.cloud_init_user_data,
        Some(String::from("tilde-user-data"))
    );
}
