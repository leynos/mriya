//! Unit tests for configuration and request validation.

use mriya::{InstanceRequest, ScalewayConfig};

fn make_valid_config() -> ScalewayConfig {
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
fn instance_request_validation_rejects_empty_fields() {
    let error = InstanceRequest::builder()
        .build()
        .expect_err("validation should fail");
    assert_eq!(error.to_string(), "missing or empty field: image_label");
}

#[test]
fn instance_request_validation_rejects_other_empty_fields() {
    let baseline = InstanceRequest::builder()
        .image_label("ubuntu-22-04")
        .instance_type("DEV1-S")
        .zone("fr-par-1")
        .project_id("11111111-2222-3333-4444-555555555555")
        .architecture("x86_64")
        .build()
        .expect("baseline request should be valid");

    let cases = [
        (
            "instance_type",
            InstanceRequest {
                instance_type: String::new(),
                ..baseline.clone()
            },
        ),
        (
            "zone",
            InstanceRequest {
                zone: String::new(),
                ..baseline.clone()
            },
        ),
        (
            "project_id",
            InstanceRequest {
                project_id: String::new(),
                ..baseline.clone()
            },
        ),
        (
            "architecture",
            InstanceRequest {
                architecture: String::new(),
                ..baseline.clone()
            },
        ),
    ];

    for (field, request) in cases {
        let error = request
            .validate()
            .expect_err(&format!("validation should fail for empty {field}"));
        assert_eq!(
            error.to_string(),
            format!("missing or empty field: {field}")
        );
    }
}

#[test]
fn instance_request_trims_whitespace() {
    let error = InstanceRequest::builder()
        .image_label("  ")
        .instance_type("  ")
        .zone("  ")
        .project_id("  ")
        .architecture("  ")
        .build()
        .expect_err("whitespace-only fields should be empty");
    assert_eq!(error.to_string(), "missing or empty field: image_label");
}

#[test]
fn config_validation_rejects_missing_secret() {
    let cfg = ScalewayConfig {
        secret_key: String::new(),
        ..make_valid_config()
    };

    let error = cfg.validate().expect_err("secret is required");
    assert_eq!(
        error.to_string(),
        "missing configuration field: SCW_SECRET_KEY"
    );
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
        make_valid_config(),
        |cfg| cfg.default_project_id.clear(),
        "missing configuration field: SCW_DEFAULT_PROJECT_ID",
    );

    assert_missing(
        make_valid_config(),
        |cfg| cfg.default_image.clear(),
        "missing configuration field: default_image",
    );

    assert_missing(
        make_valid_config(),
        |cfg| cfg.default_instance_type.clear(),
        "missing configuration field: default_instance_type",
    );

    assert_missing(
        make_valid_config(),
        |cfg| cfg.default_zone.clear(),
        "missing configuration field: default_zone",
    );

    assert_missing(
        make_valid_config(),
        |cfg| cfg.default_architecture.clear(),
        "missing configuration field: default_architecture",
    );
}

#[test]
fn config_as_request_produces_valid_request() {
    let cfg = make_valid_config();
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
