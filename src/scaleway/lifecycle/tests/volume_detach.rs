//! Loopback tests for the dedicated Instances detach endpoint.
//!
//! Detach names one volume and must keep the root entry. The response is
//! verified before the Block Storage status wait, so a still-attached cache
//! cannot look detached.

use anyhow::Result;
use rstest::rstest;

use super::{
    backend_fixture,
    handle,
    http_stub::{request_body, request_url, with_stub},
    volume_attach::{
        CACHE_ID,
        available_volume,
        detached_server,
        expected_map,
        rejected,
        server_with,
    },
};
use crate::{
    backend::InstanceHandle,
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

/// Scenario: detaching the cache.
///
/// Invariant: the POST names only the volume, the returned map keeps the root,
/// and the backend then waits for `available`.
#[rstest]
#[tokio::test]
async fn detach_volume_posts_the_id_and_waits_for_available(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![detached_server(), available_volume()],
        async |backend| backend.detach_volume(&handle, CACHE_ID).await,
    )
    .await?;
    result?;

    let detach = requests.first().expect("detach request missing");
    assert!(detach.starts_with("POST "), "{detach}");
    assert_eq!(
        request_url(detach)?.path(),
        "/instance/v1/zones/zone/servers/id/detach-volume"
    );
    assert_eq!(
        request_body(detach)?,
        serde_json::json!({"volume_id": CACHE_ID})
    );

    let status_read = requests.get(1).expect("status read missing");
    assert_eq!(
        request_url(status_read)?.path(),
        format!("/block/v1/zones/zone/volumes/{CACHE_ID}")
    );
    Ok(())
}

/// Scenario: the cache survives a detachment response.
///
/// Invariant: the mismatch fails as `VolumeDetachFailed` and no status read
/// follows, so a still-attached cache cannot look detached.
#[rstest]
#[tokio::test]
async fn detach_volume_rejects_a_map_that_still_holds_the_cache(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let reply = server_with(&serde_json::json!({
        "0": {"id": "root-id", "volume_type": "sbs_volume"},
        "1": {"id": CACHE_ID, "volume_type": "sbs_volume"},
    }));
    let (result, requests) = with_stub(backend_fixture, vec![reply], async |backend| {
        backend.detach_volume(&handle, CACHE_ID).await
    })
    .await?;

    let error = result.expect_err("a surviving cache entry must fail");
    let rendered = format!("{error:?}");
    assert!(
        matches!(
            error,
            ScalewayBackendError::VolumeDetachFailed { ref message, .. }
                if message.contains("still present")
        ),
        "{rendered}"
    );
    assert_eq!(requests.len(), 1);
    Ok(())
}

/// Scenario: the provider rejects a detachment.
///
/// Invariant: the error names both resources and carries the provider body.
#[rstest]
#[tokio::test]
async fn detach_volume_reports_a_provider_rejection(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, _) = with_stub(backend_fixture, vec![rejected()], async |backend| {
        backend.detach_volume(&handle, CACHE_ID).await
    })
    .await?;

    assert_eq!(
        result.expect_err("a rejected detach must fail"),
        ScalewayBackendError::VolumeDetachFailed {
            volume_id: String::from(CACHE_ID),
            instance_id: String::from("id"),
            message: rejected().body,
        }
    );
    Ok(())
}

/// Scenario: a detach response without a root entry.
///
/// Invariant: the contract failure names the missing root rather than treating
/// an empty map as success.
#[rstest]
#[tokio::test]
async fn detach_volume_rejects_a_missing_root_entry(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let reply = server_with(&serde_json::json!({}));
    let (result, _) = with_stub(backend_fixture, vec![reply], async |backend| {
        backend.detach_volume(&handle, CACHE_ID).await
    })
    .await?;

    let error = result.expect_err("a missing root must fail");
    let rendered = format!("{error:?}");
    assert!(
        matches!(
            error,
            ScalewayBackendError::VolumeDetachFailed { ref message, .. }
                if message.contains("root volume")
        ),
        "{rendered}"
    );
    Ok(())
}

/// Scenario: the cache entry is present but typed as another volume type.
///
/// Invariant: attach refuses a response that did not declare `sbs_volume`.
#[rstest]
#[tokio::test]
async fn attach_volume_requires_the_sbs_volume_type(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let reply = server_with(&serde_json::json!({
        "0": {"id": "root-id", "volume_type": "sbs_volume"},
        "1": {"id": CACHE_ID, "volume_type": "l_ssd"},
    }));
    let (result, _) = with_stub(backend_fixture, vec![reply], async |backend| {
        backend
            .attach_volume(&handle, CACHE_ID, &expected_map())
            .await
    })
    .await?;

    let error = result.expect_err("a non-SBS entry must fail");
    let rendered = format!("{error:?}");
    assert!(
        matches!(
            error,
            ScalewayBackendError::VolumeAttachmentFailed { ref message, .. }
                if message.contains("l_ssd")
        ),
        "{rendered}"
    );
    Ok(())
}
