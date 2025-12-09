//! Unit tests for configuration and request validation.

use mriya::{ScalewayConfig, config::ConfigError};
use rstest::*;

#[fixture]
fn valid_config() -> ScalewayConfig {
    ScalewayConfig {
        access_key: Some(String::from("SCWACCESSKEYEXAMPLE")),
        secret_key: String::from("SCWSECRETKEYEXAMPLE"),
        default_organization_id: None,
        default_project_id: String::from("11111111-2222-3333-4444-555555555555"),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from("DEV1-S"),
        default_image: String::from("ubuntu-22-04"),
        default_architecture: String::from("x86_64"),
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
}
