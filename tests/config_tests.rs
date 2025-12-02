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
fn config_validation_rejects_missing_secret() {
    let cfg = ScalewayConfig {
        secret_key: String::new(),
        ..valid_config()
    };

    let error = cfg.validate().expect_err("secret is required");
    assert!(matches!(error, ConfigError::MissingField(field) if field == "SCW_SECRET_KEY"));
}

#[test]
fn config_validation_rejects_other_fields() {
    fn assert_missing(
        mut cfg: ScalewayConfig,
        mutate: impl FnOnce(&mut ScalewayConfig),
        expected: &str,
    ) {
        mutate(&mut cfg);
        let error = cfg.validate().expect_err("validation should fail");
        assert_eq!(error.to_string(), expected);
    }

    assert_missing(
        valid_config(),
        |cfg| cfg.default_project_id.clear(),
        "missing configuration field: SCW_DEFAULT_PROJECT_ID",
    );

    assert_missing(
        valid_config(),
        |cfg| cfg.default_image.clear(),
        "missing configuration field: SCW_DEFAULT_IMAGE",
    );

    assert_missing(
        valid_config(),
        |cfg| cfg.default_instance_type.clear(),
        "missing configuration field: SCW_DEFAULT_INSTANCE_TYPE",
    );

    assert_missing(
        valid_config(),
        |cfg| cfg.default_zone.clear(),
        "missing configuration field: SCW_DEFAULT_ZONE",
    );

    assert_missing(
        valid_config(),
        |cfg| cfg.default_architecture.clear(),
        "missing configuration field: SCW_DEFAULT_ARCHITECTURE",
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
