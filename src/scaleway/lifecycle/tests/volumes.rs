//! Tests through the real volume lifecycle wrappers and a loopback Scaleway
//! API.
//!
//! Each case points the backend at [`StubApi`] and asserts the request the
//! provider would receive, so a hard-coded API base, a wrong index in the
//! attachment payload, or a detachment that forgets to exclude the preserved
//! volume fails here rather than in a live account.

use anyhow::{Context, Result};
use rstest::rstest;

use super::{
    backend_fixture,
    handle,
    http_stub::{Reply, only_request, request_body, request_url, with_stub},
    ledger,
};
use crate::{
    backend::InstanceHandle,
    ownership::{ResourceLedger, StorageFamily},
    scaleway::{ScalewayBackend, ScalewayBackendError, lifecycle::volumes::CacheAttachment},
    volume::{VolumeHandle, VolumeRequest},
};

/// A provider response confirming a volume patch.
fn patched() -> Reply {
    Reply {
        status: 200,
        body: String::from("{}"),
    }
}

/// A provider response confirming a volume creation.
fn created_volume() -> Reply {
    Reply {
        status: 201,
        body: String::from(r#"{"volume":{"id":"vol-created","zone":"zone"}}"#),
    }
}

/// A structured provider denial shared by the rejection cases.
fn rejected() -> Reply {
    Reply {
        status: 404,
        body: String::from(r#"{"type":"denied","message":"no such volume"}"#),
    }
}

/// A provider response rejecting the volume update for a reason other than
/// absence; a 404 would mean the server is gone instead.
fn rejected_update() -> Reply {
    Reply {
        status: 400,
        body: String::from(r#"{"type":"denied","message":"volume is busy"}"#),
    }
}

/// The single-server instance fixture whose `"0"` index is the root volume.
fn attached_instance() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/detach-instance.json").to_owned(),
    }
}

/// The attached fixture with `volume_id` removed from the volume map.
fn instance_without(volume_id: &str) -> Result<Reply> {
    let mut instance: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/detach-instance.json"))
            .context("detach fixture is not JSON")?;
    let volumes = instance
        .pointer_mut("/server/volumes")
        .context("detach fixture has no volumes")?
        .as_object_mut()
        .context("detach fixture volumes are not an object")?;
    let index = volumes
        .iter()
        .find(|(_, volume)| volume.get("id").and_then(serde_json::Value::as_str) == Some(volume_id))
        .map(|(index, _)| index.clone())
        .context("fixture does not carry the volume")?;
    volumes.remove(&index);
    Ok(Reply {
        status: 200,
        body: serde_json::to_string(&instance)?,
    })
}

/// Scenario: attaching a cache volume to a stopped instance.
///
/// Invariant: the PATCH targets the configured instance and sends the root
/// volume as the bootable entry at index `"0"` and the cache volume,
/// non-bootable, at index `"1"`.
#[rstest]
#[tokio::test]
async fn attach_volume_patches_root_and_cache_indices(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![patched()], async |backend| {
        backend
            .attach_volume(
                &handle,
                CacheAttachment {
                    volume_id: "vol-cache",
                    root_volume_id: "vol-root",
                    family: StorageFamily::Instance,
                },
            )
            .await
    })
    .await?;
    result?;
    let request = only_request(&requests)?;
    assert!(request.starts_with("PATCH "), "{request}");
    assert_eq!(
        request_url(request)?.path(),
        "/instance/v1/zones/zone/servers/id"
    );
    assert_eq!(
        request_body(request)?,
        serde_json::json!({
            "volumes": {
                "0": {"id": "vol-root", "boot": true},
                "1": {"id": "vol-cache"},
            }
        })
    );
    Ok(())
}

/// Scenario: attaching a volume when the root identifier is blank.
///
/// Invariant: a blank root fails as `VolumeNotFound` before any request is
/// sent, so a caller cannot patch away an unknown root volume.
#[rstest]
#[tokio::test]
async fn attach_volume_rejects_a_missing_root_volume(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![], async |backend| {
        backend
            .attach_volume(
                &handle,
                CacheAttachment {
                    volume_id: "vol-cache",
                    root_volume_id: "   ",
                    family: StorageFamily::Instance,
                },
            )
            .await
    })
    .await?;
    let error = result.expect_err("a blank root volume must not be attached");
    assert!(
        matches!(error, ScalewayBackendError::VolumeNotFound { ref volume_id, .. } if volume_id == "0"),
        "{error:?}"
    );
    assert!(requests.is_empty(), "{requests:?}");
    Ok(())
}

/// Scenario: detaching a preserved cache volume.
///
/// Invariant: the backend reads the instance, patches a map that keeps every
/// other reported volume, and re-reads the instance to confirm the volume is
/// gone.
#[rstest]
#[tokio::test]
async fn detach_volumes_preserves_every_other_volume(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![
            attached_instance(),
            patched(),
            instance_without("vol-cache")?,
        ],
        async |backend| {
            backend
                .detach_volumes(&handle, &[String::from("vol-cache")])
                .await
        },
    )
    .await?;
    result?;
    assert_eq!(requests.len(), 3);
    let read = requests.first().context("instance read missing")?;
    assert!(read.starts_with("GET "), "{read}");
    assert!(
        read.contains("x-auth-token: dummy"),
        "instance read must carry the configured token: {read}"
    );
    let patch = requests.get(1).context("detachment patch missing")?;
    assert!(patch.starts_with("PATCH "), "{patch}");
    assert_eq!(
        request_body(patch)?,
        serde_json::json!({
            "volumes": { "0": {"id": "vol-root", "boot": true} }
        })
    );
    let verify = requests.get(2).context("verification read missing")?;
    assert!(verify.starts_with("GET "), "{verify}");
    Ok(())
}

/// Scenario: the provider accepts the PATCH but keeps the volume attached.
///
/// Invariant: verification re-reads the instance and reports
/// `VolumeDetachFailed`, so a successful status alone never counts as a
/// detached preserved volume.
#[rstest]
#[tokio::test]
async fn detach_volumes_rejects_an_unverified_detachment(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![attached_instance(), patched(), attached_instance()],
        async |backend| {
            backend
                .detach_volumes(&handle, &[String::from("vol-cache")])
                .await
        },
    )
    .await?;
    let error = result.expect_err("a still-attached volume must not verify");
    assert!(
        matches!(error, ScalewayBackendError::VolumeDetachFailed { ref volume_id, .. } if volume_id == "vol-cache"),
        "{error:?}"
    );
    assert_eq!(requests.len(), 3);
    Ok(())
}

/// Scenario: the provider rejects the detachment PATCH.
///
/// Invariant: the rejection names the volume and instance, and no verification
/// read follows.
#[rstest]
#[tokio::test]
async fn detach_volumes_reports_the_operation_error(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![attached_instance(), rejected_update()],
        async |backend| {
            backend
                .detach_volumes(&handle, &[String::from("vol-cache")])
                .await
        },
    )
    .await?;
    let error = result.expect_err("a rejected detachment must fail");
    assert!(
        matches!(
            error,
            ScalewayBackendError::VolumeDetachFailed {
                ref volume_id,
                ref instance_id,
                ..
            } if volume_id == "vol-cache" && instance_id == "id"
        ),
        "{error:?}"
    );
    assert_eq!(requests.len(), 2);
    Ok(())
}

/// Scenario: the provider rejects a creation.
///
/// Invariant: the rejection names the requested volume and zone plus the
/// provider's own message.
#[rstest]
#[tokio::test]
async fn create_volume_reports_the_creation_error(backend_fixture: ScalewayBackend) -> Result<()> {
    let ledger = ledger();
    let (result, requests) = with_stub(backend_fixture, vec![rejected()], async |backend| {
        let wanted = VolumeRequest::new("cache", 10_000_000_000, "zone", "proj");
        backend.create_volume(&wanted, &ledger).await
    })
    .await?;
    let error = result.expect_err("a rejected creation must fail");
    assert!(
        matches!(error, ScalewayBackendError::VolumeCreateFailed { ref name, ref zone, .. } if name == "cache" && zone == "zone"),
        "{error:?}"
    );
    assert_eq!(requests.len(), 1);
    Ok(())
}

/// Scenario: creating a Block Storage volume.
///
/// Invariant: the POST carries the requested name, size, project, and tags
/// (including the allocation tag), the provider's identifiers come back as the
/// handle, and the volume is recorded as owned.
#[rstest]
#[tokio::test]
async fn create_volume_posts_the_request_and_records_ownership(
    backend_fixture: ScalewayBackend,
) -> Result<()> {
    let ledger: ResourceLedger = ledger();
    let tag = ledger.tag();
    let (result, requests) = with_stub(backend_fixture, vec![created_volume()], async |backend| {
        let wanted = VolumeRequest::new("cache", 10_000_000_000, "zone", "proj");
        backend.create_volume(&wanted, &ledger).await
    })
    .await?;
    let volume: VolumeHandle = result?;
    assert_eq!(volume.id, "vol-created");
    assert_eq!(volume.zone, "zone");
    assert_eq!(
        ledger.entries().first().map(|entry| entry.resource.id()),
        Some("vol-created")
    );
    assert_eq!(
        ledger.entries().first().map(|entry| entry.disposition),
        Some(crate::ownership::Disposition::Owned),
        "a created volume must be recorded as owned so cleanup deletes it"
    );
    let request = only_request(&requests)?;
    assert!(request.starts_with("POST "), "{request}");
    assert_eq!(
        request_url(request)?.path(),
        "/instance/v1/zones/zone/volumes"
    );
    assert_eq!(
        request_body(request)?,
        serde_json::json!({
            "name": "cache",
            "size": 10_000_000_000u64,
            "volume_type": "b_ssd",
            "project": "proj",
            "tags": ["mriya", "cache", tag.as_str()],
        })
    );
    Ok(())
}
