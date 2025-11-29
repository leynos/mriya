//! Unit tests for configuration and request validation.

use mriya::{InstanceRequest, ScalewayConfig};

#[test]
fn instance_request_validation_rejects_empty_fields() {
    let request = InstanceRequest::new("", "", "", "", None, "");

    let error = request.validate().expect_err("validation should fail");
    assert_eq!(error.to_string(), "missing or empty field: image_label");
}

#[test]
fn config_validation_rejects_missing_secret() {
    let cfg = ScalewayConfig {
        access_key: None,
        secret_key: String::new(),
        default_organization_id: None,
        default_project_id: String::from("project"),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from("DEV1-S"),
        default_image: String::from("Ubuntu 24.04 Noble Numbat"),
        default_architecture: String::from("x86_64"),
    };

    let error = cfg.validate().expect_err("secret is required");
    assert_eq!(
        error.to_string(),
        "missing configuration field: SCW_SECRET_KEY"
    );
}
