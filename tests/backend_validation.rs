//! Unit tests for backend request construction and validation.

#[path = "common/test_constants.rs"]
mod test_constants;

use test_constants::DEFAULT_INSTANCE_TYPE;

use mriya::{InstanceRequest, backend::BackendError};

#[test]
fn validate_rejects_empty_fields() {
    let error = InstanceRequest::builder()
        .build()
        .expect_err("validation should fail");
    assert_eq!(error, BackendError::Validation(String::from("image_label")));
}

#[test]
fn validate_rejects_other_missing_fields() {
    let base = InstanceRequest::builder()
        .image_label("ubuntu-22-04")
        .instance_type(DEFAULT_INSTANCE_TYPE)
        .zone("fr-par-1")
        .project_id("project-id")
        .architecture("x86_64")
        .build()
        .expect("baseline request should be valid");

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
    let error = InstanceRequest::builder()
        .image_label("  ")
        .instance_type("  ")
        .zone("  ")
        .project_id("  ")
        .architecture("  ")
        .build()
        .expect_err("whitespace-only values should fail");
    assert_eq!(error, BackendError::Validation(String::from("image_label")));
}

#[test]
fn validate_rejects_empty_cloud_init_user_data() {
    let error = InstanceRequest::builder()
        .image_label("ubuntu-22-04")
        .instance_type(DEFAULT_INSTANCE_TYPE)
        .zone("fr-par-1")
        .project_id("project-id")
        .architecture("x86_64")
        .cloud_init_user_data(Some(String::from("   ")))
        .build()
        .expect_err("empty cloud-init should fail");
    assert_eq!(
        error,
        BackendError::Validation(String::from("cloud_init_user_data"))
    );
}
