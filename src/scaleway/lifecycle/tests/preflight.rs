//! Loopback tests for cache preflight refusals.
//!
//! Every refusal must be typed and must stop before the server POST, so an
//! invalid volume cannot allocate a paid server.

use anyhow::Result;
use rstest::rstest;

use super::{
    backend_fixture,
    base_request,
    http_stub::{Reply, with_stub},
    provision::{CACHE_ID, at, in_use_volume, request_with_volume, with_project},
};
use crate::{
    backend::InstanceRequest,
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

/// One preflight refusal and the reply that provokes it.
#[derive(Clone, Copy)]
enum PreflightCase {
    MissingZone,
    ProjectMismatch,
    BlankProject,
    Legacy,
    Attached,
    Creating,
}

impl PreflightCase {
    /// The single reply the preflight lookup receives.
    fn reply(self) -> Reply {
        match self {
            Self::MissingZone => Reply {
                status: 404,
                body: String::from(r#"{"type":"not_found","message":"no such volume"}"#),
            },
            Self::ProjectMismatch => with_project("other-project"),
            Self::BlankProject => with_project(""),
            Self::Legacy => Reply {
                status: 200,
                body: include_str!("fixtures/block-volume-legacy.json").to_owned(),
            },
            Self::Attached => in_use_volume(),
            Self::Creating => Reply {
                status: 200,
                body: include_str!("fixtures/block-volume-creating.json").to_owned(),
            },
        }
    }

    /// The typed error the preflight must report.
    fn expected(self) -> ScalewayBackendError {
        match self {
            Self::MissingZone => ScalewayBackendError::VolumeNotInZone {
                volume_id: String::from(CACHE_ID),
                zone: String::from("zone"),
            },
            Self::ProjectMismatch => ScalewayBackendError::VolumeProjectMismatch {
                volume_id: String::from(CACHE_ID),
                expected_project_id: String::from("proj"),
                actual_project_id: String::from("other-project"),
            },
            Self::BlankProject => ScalewayBackendError::VolumeProjectMismatch {
                volume_id: String::from(CACHE_ID),
                expected_project_id: String::from("proj"),
                actual_project_id: String::new(),
            },
            Self::Legacy => ScalewayBackendError::VolumeLegacy {
                volume_id: String::from(CACHE_ID),
            },
            Self::Attached => ScalewayBackendError::VolumeAlreadyAttached {
                volume_id: String::from(CACHE_ID),
                references: String::from("instance_server:server-id (attached)"),
                recovery: format!(
                    "detach it with `scw instance server detach-volume server-id=server-id volume-id={CACHE_ID} zone=zone`"
                ),
            },
            Self::Creating => ScalewayBackendError::VolumeUnavailable {
                volume_id: String::from(CACHE_ID),
                status: String::from("creating"),
                guidance: String::from("wait until it is 'available' before using it as a cache"),
            },
        }
    }
}

/// Scenario: the configured volume fails preflight.
///
/// Invariant: every refusal is typed and no server POST follows, so an invalid
/// volume cannot allocate a paid server.
#[rstest]
#[case::missing_zone(PreflightCase::MissingZone)]
#[case::project_mismatch(PreflightCase::ProjectMismatch)]
#[case::blank_project(PreflightCase::BlankProject)]
#[case::legacy(PreflightCase::Legacy)]
#[case::attached(PreflightCase::Attached)]
#[case::creating(PreflightCase::Creating)]
#[tokio::test]
async fn create_with_image_id_rejects_before_the_server_post(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
    #[case] case: PreflightCase,
) -> Result<()> {
    let request = request_with_volume(&base_request);
    let (result, requests) = with_stub(backend_fixture, vec![case.reply()], async |backend| {
        backend.create_with_image_id(&request, "img-test").await
    })
    .await?;

    assert_eq!(result.expect_err("preflight must fail"), case.expected());
    assert_eq!(requests.len(), 1);
    assert!(
        at(&requests, 0).starts_with("GET /block/v1/"),
        "{}",
        at(&requests, 0)
    );
    Ok(())
}
