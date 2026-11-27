//! Tests for image resolution and selection logic.

use std::{cell::Cell, rc::Rc};

use rstest::rstest;
use scaleway_rs::ScalewayImage;

use crate::{
    boot_volume::ImageStorage,
    scaleway::{ScalewayBackend, ScalewayBackendError, lifecycle::image::ImageQuery},
};

/// The storage kinds an instance type offering both would publish.
fn both_storages() -> ImageStorage { ImageStorage::new(true, true) }

/// A query for an instance offering both storage kinds.
fn query(request: &crate::backend::InstanceRequest) -> ImageQuery<'_> {
    ImageQuery::new(request, both_storages())
}

/// A query for an instance offering `supported`.
fn query_with(
    request: &crate::backend::InstanceRequest,
    supported: ImageStorage,
) -> ImageQuery<'_> {
    ImageQuery::new(request, supported)
}

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

    let root = ScalewayBackend::select_image_id(images, &request).expect("image selected");
    assert_eq!(root.id, "newest");
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
            query(&request),
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

    assert_eq!(result.id, "project-img");
    assert!(project_called.get());
    assert!(!public_called.get(), "public lookup should not be needed");
}

#[tokio::test]
async fn resolve_image_id_falls_back_to_public() {
    let request = super::base_request();
    let backend = super::backend_fixture();
    let result = backend
        .resolve_image_id_with(
            query(&request),
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

    assert_eq!(result.id, "public-img");
}

#[tokio::test]
async fn resolve_image_id_propagates_errors() {
    let request = super::base_request();
    let backend = super::backend_fixture();
    let err = backend
        .resolve_image_id_with(
            query(&request),
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

/// Builds an image whose root volume reports `volume_type` and `size`.
fn image_with_root(
    id: &'static str,
    creation_date: &'static str,
    volume_type: &'static str,
    size: u64,
) -> scaleway_rs::ScalewayImage {
    let mut built = super::image(super::ImageSpec {
        id,
        arch: "x86_64",
        state: "available",
        creation_date,
    });
    built.root_volume = scaleway_rs::ScalewayImageRootVolume {
        id: String::new(),
        name: String::new(),
        size,
        volume_type: volume_type.to_owned(),
    };
    built
}

/// The resolver reports the chosen image's root size and storage kinds, which
/// is what the plan is resolved against.
#[tokio::test]
async fn resolve_image_id_reports_the_root_facts() {
    let request = super::base_request();
    let backend = super::backend_fixture();

    let root = backend
        .resolve_image_id_with(
            query(&request),
            || async { Ok(Vec::new()) },
            || async {
                Ok(vec![image_with_root(
                    "public-img",
                    "2025-01-01T00:00:00Z",
                    "sbs_snapshot",
                    10_000_000_000,
                )])
            },
        )
        .await
        .expect("the image should resolve");

    assert_eq!(root.id, "public-img");
    assert_eq!(root.size_bytes, 10_000_000_000);
    assert_eq!(root.volume_type, "sbs_snapshot");
    assert!(root.storage.sbs);
    assert!(!root.storage.local);
}

/// An image-label vocabulary the backend does not model is refused by name
/// rather than assumed compatible.
#[tokio::test]
async fn resolve_image_id_refuses_an_unmodelled_root_type() {
    let request = super::base_request();
    let backend = super::backend_fixture();

    let error = backend
        .resolve_image_id_with(
            query(&request),
            || async { Ok(Vec::new()) },
            || async {
                Ok(vec![image_with_root(
                    "public-img",
                    "2025-01-01T00:00:00Z",
                    "scratch",
                    10_000_000_000,
                )])
            },
        )
        .await
        .expect_err("a scratch root must be refused");

    assert!(
        matches!(
            error,
            ScalewayBackendError::ImageStorageIncompatible(ref details)
                if details.observed == "scratch"
        ),
        "{error:?}"
    );
}

/// Requesting local storage keeps only images whose root can back it, so the
/// newest compatible image wins rather than the newest image overall.
#[tokio::test]
async fn local_request_selects_the_newest_compatible_image() {
    let mut request = super::base_request();
    request.boot_volume = crate::boot_volume::BootVolumeRequest {
        volume_type: Some(crate::boot_volume::BootVolumeType::Local),
        size_gb: None,
    };
    let backend = super::backend_fixture();

    let root = backend
        .resolve_image_id_with(
            query(&request),
            || async { Ok(Vec::new()) },
            || async {
                Ok(vec![
                    image_with_root("local-old", "2025-01-01T00:00:00Z", "l_ssd", 10_000_000_000),
                    image_with_root(
                        "sbs-newest",
                        "2026-01-01T00:00:00Z",
                        "sbs_snapshot",
                        10_000_000_000,
                    ),
                ])
            },
        )
        .await
        .expect("the image should resolve");

    assert_eq!(
        root.id, "local-old",
        "the SBS-only image is newer but cannot back a local boot volume"
    );
}

/// When every candidate exists but none can back the requested storage, the
/// error names the types that were observed instead of claiming no image
/// matched the label.
#[tokio::test]
async fn incompatible_candidates_report_the_observed_types() {
    let mut request = super::base_request();
    request.boot_volume = crate::boot_volume::BootVolumeRequest {
        volume_type: Some(crate::boot_volume::BootVolumeType::Local),
        size_gb: None,
    };
    let backend = super::backend_fixture();

    let error = backend
        .resolve_image_id_with(
            query(&request),
            || async { Ok(Vec::new()) },
            || async {
                Ok(vec![
                    image_with_root("sbs-1", "2025-01-01T00:00:00Z", "sbs_snapshot", 1),
                    image_with_root("sbs-2", "2026-01-01T00:00:00Z", "sbs_volume", 1),
                ])
            },
        )
        .await
        .expect_err("no candidate can back a local boot volume");

    let ScalewayBackendError::ImageStorageIncompatible(ref details) = error else {
        panic!("expected a storage-incompatible error, got {error:?}");
    };
    assert_eq!(
        details.requested,
        Some(crate::boot_volume::BootVolumeType::Local)
    );
    assert!(details.observed.contains("sbs_snapshot"), "{details:?}");
    assert!(details.observed.contains("sbs_volume"), "{details:?}");
}

/// Each unset-type row states the storage an instance offers and the image the
/// lookup must select.
///
/// With no type named, the newest image wins when the instance can use it, and
/// an image the instance cannot boot from is skipped even when newer.
#[rstest]
#[case::both_kinds_take_the_newest(both_storages(), "local-newest")]
#[case::no_local_storage_skips_a_local_image(ImageStorage::new(false, true), "sbs-older")]
#[tokio::test]
async fn unset_type_selects_an_image_the_instance_can_boot(
    #[case] supported: ImageStorage,
    #[case] expected: &str,
) {
    let request = super::base_request();
    let backend = super::backend_fixture();

    let root = backend
        .resolve_image_id_with(
            query_with(&request, supported),
            || async { Ok(Vec::new()) },
            || async {
                Ok(vec![
                    image_with_root("local-newest", "2026-01-01T00:00:00Z", "l_ssd", 1),
                    image_with_root("sbs-older", "2025-01-01T00:00:00Z", "sbs_snapshot", 1),
                ])
            },
        )
        .await
        .expect("an image the instance can boot should resolve");

    assert_eq!(root.id, expected);
}
