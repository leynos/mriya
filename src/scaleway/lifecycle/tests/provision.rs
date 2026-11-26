//! Loopback tests for orchestrated creation with a cache volume.
//!
//! Creation validates the configured volume before the server POST, attaches
//! it through the dedicated endpoint, and powers the server on only after the
//! attachment is confirmed. Any post-POST failure rolls the server back; a
//! rollback that cannot detach the cache retains the server instead of letting
//! the provider delete the cache with it.

use anyhow::Result;
use rstest::rstest;

use super::{
    backend_fixture,
    base_request,
    http_stub::{Reply, request_body, request_url, with_stub},
};
use crate::{
    backend::InstanceRequest,
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

/// Cache volume identifier used by the creation cases.
const CACHE_ID: &str = "11111111-1111-1111-1111-111111111111";

/// A successful server creation carrying one root volume.
fn created_server() -> Reply {
    Reply {
        status: 201,
        body: include_str!("fixtures/create-instance.json").to_owned(),
    }
}

/// The Block Storage status before attachment.
fn available_volume() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/block-volume-available.json").to_owned(),
    }
}

/// The Block Storage status after attachment.
fn in_use_volume() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/block-volume-in-use.json").to_owned(),
    }
}

/// A successful attach response carrying the cache at index "1".
fn attached_server() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/attach-server.json").to_owned(),
    }
}

/// A successful detach response keeping only the root volume.
fn detached_server() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/detach-server.json").to_owned(),
    }
}

/// A successful server action.
fn action_ok() -> Reply {
    Reply {
        status: 200,
        body: String::from(r#"{"task":{}}"#),
    }
}

/// A rejected power-on action.
fn action_rejected() -> Reply {
    Reply {
        status: 400,
        body: String::from(r#"{"type":"invalid_arguments","message":"poweron rejected"}"#),
    }
}

/// A rejected attach request.
fn attach_rejected() -> Reply {
    Reply {
        status: 400,
        body: String::from(r#"{"type":"invalid_arguments","message":"volume is busy"}"#),
    }
}

/// A rejected detach request.
fn detach_rejected() -> Reply {
    Reply {
        status: 400,
        body: String::from(r#"{"type":"invalid_arguments","message":"volume is busy"}"#),
    }
}

/// A stopped server, as seen while tear down powers it off.
fn stopped_server() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/create-instance.json").to_owned(),
    }
}

/// A successful server deletion.
fn deleted() -> Reply {
    Reply {
        status: 204,
        body: String::new(),
    }
}

/// An empty server listing, ending the disappearance poll.
fn gone() -> Reply {
    Reply {
        status: 200,
        body: String::from(r#"{"servers":[]}"#),
    }
}

/// Returns the base request with the cache volume configured.
fn request_with_volume(base: &InstanceRequest) -> InstanceRequest {
    let mut request = base.clone();
    request.volume_id = Some(String::from(CACHE_ID));
    request
}

/// Returns the captured request at `index`.
fn at(requests: &[String], index: usize) -> &String {
    requests
        .get(index)
        .expect("expected a captured request at this index")
}

/// Builds a volume reply whose project differs from the configured one.
fn with_project(project: &str) -> Reply {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/block-volume-available.json"))
            .expect("volume fixture is JSON");
    value["project_id"] = serde_json::Value::String(project.to_owned());
    Reply {
        status: 200,
        body: value.to_string(),
    }
}

/// One preflight refusal and the reply that provokes it.
#[derive(Clone, Copy)]
enum PreflightCase {
    MissingZone,
    ProjectMismatch,
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
            Self::Legacy => ScalewayBackendError::VolumeLegacy {
                volume_id: String::from(CACHE_ID),
            },
            Self::Attached => ScalewayBackendError::VolumeAlreadyAttached {
                volume_id: String::from(CACHE_ID),
                references: String::from("instance_server:server-id (attached)"),
            },
            Self::Creating => ScalewayBackendError::VolumeUnavailable {
                volume_id: String::from(CACHE_ID),
                status: String::from("creating"),
            },
        }
    }
}

/// Scenario: creating a server with a cache volume.
///
/// Invariant: the lookup precedes the server POST, the attachment precedes the
/// power-on action, and the request order is exactly that sequence.
#[rstest]
#[tokio::test]
async fn create_with_image_id_attaches_before_power_on(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let request = request_with_volume(&base_request);
    let (result, requests) = with_stub(
        backend_fixture,
        vec![
            available_volume(),
            created_server(),
            attached_server(),
            in_use_volume(),
            action_ok(),
        ],
        async |backend| backend.create_with_image_id(&request, "img-test").await,
    )
    .await?;

    let handle = result?;
    assert_eq!(handle.id, "id");
    assert_eq!(handle.zone, "zone");
    assert_eq!(requests.len(), 5);

    assert_eq!(
        request_url(at(&requests, 0))?.path(),
        format!("/block/v1/zones/zone/volumes/{CACHE_ID}")
    );
    assert!(at(&requests, 1).starts_with("POST /instance/v1/zones/zone/servers "));
    assert_eq!(
        request_url(at(&requests, 2))?.path(),
        "/instance/v1/zones/zone/servers/id/attach-volume"
    );
    assert_eq!(
        request_body(at(&requests, 2))?,
        serde_json::json!({"volume_id": CACHE_ID, "volume_type": "sbs_volume"})
    );
    assert!(at(&requests, 3).starts_with("GET /block/v1/zones/zone/volumes/"));
    assert_eq!(
        request_url(at(&requests, 4))?.path(),
        "/instance/v1/zones/zone/servers/id/action"
    );
    assert_eq!(
        request_body(at(&requests, 4))?,
        serde_json::json!({"action": "poweron"})
    );
    Ok(())
}

/// Scenario: creating a server without a cache volume.
///
/// Invariant: no Block Storage request is made and the server is powered on
/// immediately after creation.
#[rstest]
#[tokio::test]
async fn create_with_image_id_without_a_volume_skips_the_block_api(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![created_server(), action_ok()],
        async |backend| {
            backend
                .create_with_image_id(&base_request, "img-test")
                .await
        },
    )
    .await?;

    result?;
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| !request.contains("/block/v1"))
    );
    Ok(())
}

/// Scenario: the configured volume fails preflight.
///
/// Invariant: every refusal is typed and no server POST follows, so an invalid
/// volume cannot allocate a paid server.
#[rstest]
#[case::missing_zone(PreflightCase::MissingZone)]
#[case::project_mismatch(PreflightCase::ProjectMismatch)]
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

/// Scenario: the provider rejects the attachment after the server exists.
///
/// Invariant: rollback destroys the server and returns the original attach
/// error, leaving nothing billing.
#[rstest]
#[tokio::test]
async fn rollback_destroys_the_server_after_an_attach_rejection(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let request = request_with_volume(&base_request);
    let (result, requests) = with_stub(
        backend_fixture,
        vec![
            available_volume(),
            created_server(),
            attach_rejected(),
            deleted(),
            gone(),
        ],
        async |backend| backend.create_with_image_id(&request, "img-test").await,
    )
    .await?;

    assert_eq!(
        result.expect_err("a rejected attach must fail"),
        ScalewayBackendError::VolumeAttachmentFailed {
            volume_id: String::from(CACHE_ID),
            instance_id: String::from("id"),
            message: attach_rejected().body,
        }
    );
    assert_eq!(requests.len(), 5);
    assert!(at(&requests, 3).starts_with("DELETE /instance/v1/zones/zone/servers/id "));
    assert!(at(&requests, 4).starts_with("GET /instance/v1/zones/zone/servers?"));
    Ok(())
}

/// Scenario: power-on fails after the cache was attached.
///
/// Invariant: rollback stops the server, detaches the cache, waits for it to
/// become `available`, and only then deletes the server.
#[rstest]
#[tokio::test]
async fn rollback_detaches_before_destroying_after_a_power_on_failure(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let request = request_with_volume(&base_request);
    let (result, requests) = with_stub(
        backend_fixture,
        vec![
            available_volume(),
            created_server(),
            attached_server(),
            in_use_volume(),
            action_rejected(),
            stopped_server(),
            gone(),
            detached_server(),
            available_volume(),
            deleted(),
            gone(),
        ],
        async |backend| backend.create_with_image_id(&request, "img-test").await,
    )
    .await?;

    let error = result.expect_err("a rejected power-on must fail");
    assert!(
        matches!(error, ScalewayBackendError::Provider { ref message } if message.contains("poweron rejected")),
        "{error:?}"
    );
    assert_eq!(requests.len(), 11);
    assert_eq!(
        request_url(at(&requests, 7))?.path(),
        "/instance/v1/zones/zone/servers/id/detach-volume"
    );
    assert!(at(&requests, 9).starts_with("DELETE /instance/v1/zones/zone/servers/id "));
    Ok(())
}

/// Scenario: the rollback cannot detach the cache.
///
/// Invariant: the server is retained and the returned error reports both the
/// original failure and the cleanup failure, so the cache cannot be deleted
/// with the server.
#[rstest]
#[tokio::test]
async fn rollback_retains_the_server_when_detachment_fails(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let request = request_with_volume(&base_request);
    let (result, requests) = with_stub(
        backend_fixture,
        vec![
            available_volume(),
            created_server(),
            attached_server(),
            in_use_volume(),
            action_rejected(),
            stopped_server(),
            gone(),
            detach_rejected(),
        ],
        async |backend| backend.create_with_image_id(&request, "img-test").await,
    )
    .await?;

    let error = result.expect_err("a failed rollback must fail");
    let ScalewayBackendError::RollbackFailed { original, cleanup } = &error else {
        return Err(anyhow::anyhow!("expected RollbackFailed, got {error:?}"));
    };
    assert!(
        matches!(**original, ScalewayBackendError::Provider { .. }),
        "{original:?}"
    );
    assert!(cleanup.contains("retained"), "{cleanup}");
    assert_eq!(requests.len(), 8);
    assert!(
        requests
            .iter()
            .all(|request| !request.starts_with("DELETE /instance/")),
        "a retained server must not be deleted: {requests:?}"
    );
    Ok(())
}
