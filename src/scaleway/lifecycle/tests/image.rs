//! Tests for image resolution and selection logic.

use std::cell::Cell;
use std::rc::Rc;

use scaleway_rs::ScalewayImage;

use crate::scaleway::{ScalewayBackend, ScalewayBackendError};

#[test]
fn select_image_id_returns_newest_creation_date() {
    let request = super::base_request();
    let images = vec![
        super::image(super::ImageSpec {
            id: "older",
            arch: "x86_64",
            state: "available",
            creation_date: "2025-01-01T00:00:00Z",
        }),
        super::image(super::ImageSpec {
            id: "newest",
            arch: "x86_64",
            state: "available",
            creation_date: "2025-02-01T00:00:00Z",
        }),
    ];

    let id = ScalewayBackend::select_image_id(images, &request).expect("image selected");
    assert_eq!(id, "newest");
}

#[test]
fn select_image_id_errors_on_empty() {
    let request = super::base_request();
    let images: Vec<ScalewayImage> = Vec::new();
    let err = ScalewayBackend::select_image_id(images, &request)
        .expect_err("empty candidates should fail");
    assert!(matches!(err, ScalewayBackendError::ImageNotFound { .. }));
}

#[tokio::test]
#[expect(
    clippy::excessive_nesting,
    reason = "nested async closures keep fixtures inline for readability"
)]
async fn resolve_image_id_prefers_project_results() {
    let request = super::base_request();
    let project_called = Rc::new(Cell::new(false));
    let public_called = Rc::new(Cell::new(false));

    let backend = super::backend_fixture();

    let result = backend
        .resolve_image_id_with(
            &request,
            {
                let flag = Rc::clone(&project_called);
                move || {
                    flag.set(true);
                    async {
                        Ok(vec![super::image(super::ImageSpec {
                            id: "project-img",
                            arch: "x86_64",
                            state: "available",
                            creation_date: "2025-02-01T00:00:00Z",
                        })])
                    }
                }
            },
            {
                let flag = Rc::clone(&public_called);
                move || {
                    flag.set(true);
                    async {
                        Ok(vec![super::image(super::ImageSpec {
                            id: "public-img",
                            arch: "x86_64",
                            state: "available",
                            creation_date: "2025-01-01T00:00:00Z",
                        })])
                    }
                }
            },
        )
        .await
        .expect("project image should resolve");

    assert_eq!(result, "project-img");
    assert!(project_called.get());
    assert!(!public_called.get(), "public lookup should not be needed");
}

#[tokio::test]
async fn resolve_image_id_falls_back_to_public() {
    let request = super::base_request();
    let backend = super::backend_fixture();
    let result = backend
        .resolve_image_id_with(
            &request,
            || async { Ok(Vec::new()) },
            || async {
                Ok(vec![super::image(super::ImageSpec {
                    id: "public-img",
                    arch: "x86_64",
                    state: "available",
                    creation_date: "2025-01-01T00:00:00Z",
                })])
            },
        )
        .await
        .expect("public fallback should resolve");

    assert_eq!(result, "public-img");
}

#[tokio::test]
async fn resolve_image_id_propagates_errors() {
    let request = super::base_request();
    let backend = super::backend_fixture();
    let err = backend
        .resolve_image_id_with(
            &request,
            || async {
                Err(ScalewayBackendError::Provider {
                    message: "boom".to_owned(),
                })
            },
            || async { Ok(Vec::new()) },
        )
        .await
        .expect_err("error should surface");

    assert!(matches!(err, ScalewayBackendError::Provider { message } if message == "boom"));
}
