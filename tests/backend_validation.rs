//! Unit tests for backend request construction and validation.

#[path = "common/test_constants.rs"]
mod test_constants;

use mriya::{InstanceRequest, backend::BackendError};
use test_constants::DEFAULT_INSTANCE_TYPE;

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

/// Rejects blank and non-UUID cache volume identifiers.
#[test]
fn validate_rejects_a_volume_id_that_is_not_a_canonical_uuid() {
    let base = InstanceRequest::builder()
        .image_label("ubuntu-22-04")
        .instance_type(DEFAULT_INSTANCE_TYPE)
        .zone("fr-par-1")
        .project_id("project-id")
        .architecture("x86_64")
        .build()
        .expect("baseline request should be valid");

    for value in ["", "   ", "vol-123", "11111111222233334444555555555555"] {
        let request = InstanceRequest {
            volume_id: Some(String::from(value)),
            ..base.clone()
        };
        let error = request.validate().expect_err("invalid volume_id must fail");
        assert!(
            matches!(error, BackendError::VolumeIdNotUuid { .. }),
            "{value:?}: {error:?}"
        );
    }
}

/// Accepts a canonical hyphenated UUID.
#[test]
fn validate_accepts_a_canonical_volume_id() {
    let request = InstanceRequest::builder()
        .image_label("ubuntu-22-04")
        .instance_type(DEFAULT_INSTANCE_TYPE)
        .zone("fr-par-1")
        .project_id("project-id")
        .architecture("x86_64")
        .volume_id(Some(String::from("11111111-2222-3333-4444-555555555555")))
        .build()
        .expect("a canonical UUID should pass");
    assert_eq!(
        request.volume_id.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
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
