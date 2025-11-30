//! Unit tests for backend request construction and validation.

use mriya::{InstanceRequest, backend::BackendError};

#[test]
fn validate_rejects_empty_fields() {
    let request = InstanceRequest::new("", "", "", "", None, "");
    let error = request.validate().expect_err("validation should fail");
    assert_eq!(error, BackendError::Validation(String::from("image_label")));
}

#[test]
fn validate_rejects_other_missing_fields() {
    let base = InstanceRequest::new(
        "ubuntu-22-04",
        "DEV1-S",
        "fr-par-1",
        "project-id",
        None,
        "x86_64",
    );

    let cases = [
        (
            "instance_type",
            InstanceRequest {
                instance_type: String::new(),
                ..base.clone()
            },
        ),
        (
            "zone",
            InstanceRequest {
                zone: String::new(),
                ..base.clone()
            },
        ),
        (
            "project_id",
            InstanceRequest {
                project_id: String::new(),
                ..base.clone()
            },
        ),
        (
            "architecture",
            InstanceRequest {
                architecture: String::new(),
                ..base.clone()
            },
        ),
    ];

    for (field, request) in cases {
        let error = request.validate().expect_err("field should be required");
        assert_eq!(error, BackendError::Validation(field.to_owned()));
    }
}

#[test]
fn new_trims_whitespace() {
    let request = InstanceRequest::new("  ", "  ", "  ", "  ", None, "  ");
    let error = request
        .validate()
        .expect_err("whitespace-only values should fail");
    assert_eq!(error, BackendError::Validation(String::from("image_label")));
}
