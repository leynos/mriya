//! Loopback tests for rollback after a partial server creation.
//!
//! A failed post-POST step must remove the server, and a failed detach must
//! retain it so the provider cannot delete the cache with the server.

use anyhow::Result;
use rstest::rstest;

use super::{
    backend_fixture,
    base_request,
    http_stub::{request_url, with_stub},
    provision::{
        CACHE_ID,
        action_rejected,
        at,
        attach_rejected,
        attached_server,
        available_volume,
        created_server,
        deleted,
        detach_rejected,
        detached_server,
        gone,
        in_use_volume,
        request_with_volume,
        stopped_server,
    },
};
use crate::{
    backend::InstanceRequest,
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

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
            .all(|captured| !captured.starts_with("DELETE /instance/")),
        "a retained server must not be deleted: {requests:?}"
    );
    Ok(())
}
