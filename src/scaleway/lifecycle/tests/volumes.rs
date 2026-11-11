//! Tests through the real volume lifecycle wrappers and a loopback Scaleway
//! API.
//!
//! Each case points the backend at [`StubApi`] and asserts the request the
//! provider would receive, so a hard-coded API base or a wrong index in the
//! attachment payload fails here rather than in a live account. The payload
//! assertions are the contract: index `"0"` keeps the root volume bootable and
//! index `"1"` claims the cache volume without boot.

use anyhow::{Context, Result};
use rstest::rstest;

use super::http_stub::{Reply, only_request, request_body, request_url, with_stub};
use super::{backend_fixture, handle};
use crate::backend::InstanceHandle;
use crate::scaleway::{ScalewayBackend, ScalewayBackendError};
use crate::volume::{VolumeHandle, VolumeRequest};

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

/// The single-server instance fixture whose `"0"` index is the root volume.
fn attached_instance() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/detach-instance.json").to_owned(),
    }
}

/// The same instance with every volume index removed.
fn rootless_instance() -> Result<Reply> {
    let mut instance: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/detach-instance.json"))
            .context("detach fixture is not JSON")?;
    let volumes = instance
        .pointer_mut("/server/volumes")
        .context("detach fixture has no volumes")?
        .as_object_mut()
        .context("detach fixture volumes are not an object")?;
    volumes.clear();
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
            .attach_volume(&handle, "vol-cache", String::from("vol-root"))
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
            .attach_volume(&handle, "vol-cache", String::from("   "))
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

/// One rejected volume operation and the error it must report.
///
/// Each row carries the exact provider message the stub returns, so the test
/// proves the message is carried through rather than merely that some error
/// occurred. For example, the attach row expects `VolumeAttachmentFailed`
/// naming `vol-cache` and `id`.
#[derive(Clone, Copy)]
enum Rejection {
    Attach,
    Detach,
    Create,
}

impl Rejection {
    /// Runs the rejected operation against `backend`.
    async fn run(
        self,
        backend: &ScalewayBackend,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        match self {
            Self::Attach => {
                backend
                    .attach_volume(handle, "vol-cache", String::from("vol-root"))
                    .await
            }
            Self::Detach => backend.detach_volume(handle, "vol-cache").await,
            Self::Create => backend
                .create_volume(&VolumeRequest::new("cache", 10_000_000_000, "zone", "proj"))
                .await
                .map(drop),
        }
    }

    /// The reply script that reaches this operation's rejection.
    fn script(self) -> Vec<Reply> {
        match self {
            Self::Attach | Self::Create => vec![rejected()],
            // Detaching reads the instance before it patches.
            Self::Detach => vec![attached_instance(), rejected()],
        }
    }

    /// The error this operation must report for the stub's denial.
    fn expected(self) -> ScalewayBackendError {
        match self {
            Self::Attach => ScalewayBackendError::VolumeAttachmentFailed {
                volume_id: String::from("vol-cache"),
                instance_id: String::from("id"),
                message: rejected().body,
            },
            Self::Detach => ScalewayBackendError::VolumeDetachFailed {
                volume_id: String::from("vol-cache"),
                instance_id: String::from("id"),
                message: rejected().body,
            },
            Self::Create => ScalewayBackendError::VolumeCreateFailed {
                name: String::from("cache"),
                zone: String::from("zone"),
                message: rejected().body,
            },
        }
    }
}

/// Scenario: the provider rejects an attach, detach, or create request.
///
/// Invariant: each wrapper reports the operation-specific error naming the
/// volume and instance (or name and zone) plus the provider's own message, so
/// an operator can tell which request failed and why.
#[rstest]
#[case::attach(Rejection::Attach)]
#[case::detach(Rejection::Detach)]
#[case::create(Rejection::Create)]
#[tokio::test]
async fn rejected_volume_operations_report_the_operation_error(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
    #[case] rejection: Rejection,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, rejection.script(), async |backend| {
        rejection.run(backend, &handle).await
    })
    .await?;
    let error = result.expect_err("a rejected operation must fail");
    assert_eq!(error, rejection.expected());
    assert_eq!(requests.len(), rejection.script().len());
    Ok(())
}

/// Scenario: detaching a cache volume.
///
/// Invariant: the backend reads the instance to find its current root volume,
/// then patches only index `"0"`, so detaching cannot drop the root volume.
#[rstest]
#[tokio::test]
async fn detach_volume_keeps_only_the_root_index(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![attached_instance(), patched()],
        async |backend| backend.detach_volume(&handle, "vol-cache").await,
    )
    .await?;
    result?;
    assert_eq!(requests.len(), 2);
    let read = requests.first().context("instance read missing")?;
    assert!(read.starts_with("GET "), "{read}");
    // The read is a real provider call, so it must authenticate: a loopback
    // request without the token would pass a URL-only assertion while the
    // live path returned 401.
    assert!(
        read.contains("x-auth-token: dummy"),
        "instance read must carry the configured token: {read}"
    );
    assert_eq!(
        request_url(read)?.path(),
        "/instance/v1/zones/zone/servers/id"
    );
    let patch = requests.get(1).context("detachment patch missing")?;
    assert!(patch.starts_with("PATCH "), "{patch}");
    assert_eq!(
        request_body(patch)?,
        serde_json::json!({
            "volumes": { "0": {"id": "vol-root", "boot": true} }
        })
    );
    Ok(())
}

/// Scenario: detaching when the instance reports no root volume.
///
/// Invariant: the detach stops with `VolumeNotFound` after the read and before
/// the PATCH, so an unexpected provider shape cannot clear every index.
#[rstest]
#[tokio::test]
async fn detach_volume_rejects_an_instance_without_a_root_volume(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![rootless_instance()?],
        async |backend| backend.detach_volume(&handle, "vol-cache").await,
    )
    .await?;
    let error = result.expect_err("an instance without a root volume cannot be detached");
    assert!(
        matches!(error, ScalewayBackendError::VolumeNotFound { ref volume_id, .. } if volume_id == "0"),
        "{error:?}"
    );
    assert_eq!(requests.len(), 1);
    Ok(())
}

/// Scenario: creating a Block Storage volume.
///
/// Invariant: the POST carries the requested name, size, project, and tags,
/// and the provider's identifiers come back as the handle.
#[rstest]
#[tokio::test]
async fn create_volume_posts_the_request_and_returns_the_handle(
    backend_fixture: ScalewayBackend,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![created_volume()], async |backend| {
        let wanted = VolumeRequest::new("cache", 10_000_000_000, "zone", "proj");
        backend.create_volume(&wanted).await
    })
    .await?;
    let volume: VolumeHandle = result?;
    assert_eq!(volume.id, "vol-created");
    assert_eq!(volume.zone, "zone");
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
            "tags": ["mriya", "cache"],
        })
    );
    Ok(())
}
