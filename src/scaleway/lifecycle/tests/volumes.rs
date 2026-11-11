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

use super::http_stub::{Reply, StubApi};
use super::{backend_fixture, handle};
use crate::backend::InstanceHandle;
use crate::scaleway::{ScalewayBackend, ScalewayBackendError};
use crate::volume::VolumeRequest;

/// Represents a successful PATCH that changes volumes.
fn patched() -> Reply {
    Reply {
        status: 200,
        body: String::from("{}"),
    }
}

/// Represents a successful volume creation.
fn created_volume() -> Reply {
    Reply {
        status: 201,
        body: String::from(r#"{"volume":{"id":"vol-created","zone":"zone"}}"#),
    }
}

/// Represents a structured provider rejection for error-preservation checks.
fn rejected() -> Reply {
    Reply {
        status: 404,
        body: String::from(r#"{"type":"denied","message":"no such server"}"#),
    }
}

/// Parses a captured request target so tests can inspect its path.
fn request_url(request: &str) -> Result<reqwest::Url> {
    let path = request
        .split_whitespace()
        .nth(1)
        .context("request path missing")?;
    reqwest::Url::parse(&format!("http://localhost{path}")).context("invalid request URL")
}

/// Returns the body of a captured request, which follows the blank line that
/// ends its headers.
fn request_body(request: &str) -> Result<serde_json::Value> {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .context("request body missing")?;
    serde_json::from_str(body).context("request body is not JSON")
}

/// Scenario: attaching a cache volume to a stopped instance.
///
/// Invariant: the PATCH targets the configured instance and sends the root
/// volume as the bootable entry at index `"0"` and the cache volume,
/// non-bootable, at index `"1"`.
#[rstest]
#[tokio::test]
async fn attach_volume_patches_root_and_cache_indices(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let stub = StubApi::start(vec![patched()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    backend_fixture
        .attach_volume(&handle, "vol-cache", String::from("vol-root"))
        .await?;
    let requests = stub.finish().await?;
    assert_eq!(requests.len(), 1);
    let request = requests.first().context("attachment request missing")?;
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

/// Scenario: attaching a volume when the instance snapshot has no root volume.
///
/// Invariant: an empty root identifier fails before any request is sent, so a
/// missing root volume is reported as `VolumeNotFound` rather than patched away.
#[rstest]
#[tokio::test]
async fn attach_volume_rejects_a_missing_root_volume(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let stub = StubApi::start(Vec::new()).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let error = backend_fixture
        .attach_volume(&handle, "vol-cache", String::from("   "))
        .await
        .expect_err("a blank root volume must not be attached");
    assert!(matches!(
        error,
        ScalewayBackendError::VolumeNotFound {
            ref volume_id,
            ..
        } if volume_id == "0"
    ));
    stub.finish().await?;
    Ok(())
}

/// Scenario: the provider rejects an attachment.
///
/// Invariant: a non-success response becomes `VolumeAttachmentFailed`, naming
/// the volume and carrying the provider's message.
#[rstest]
#[tokio::test]
async fn attach_volume_reports_a_provider_rejection(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let stub = StubApi::start(vec![rejected()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let error = backend_fixture
        .attach_volume(&handle, "vol-cache", String::from("vol-root"))
        .await
        .expect_err("a rejected attachment must fail");
    match error {
        ScalewayBackendError::VolumeAttachmentFailed {
            volume_id,
            instance_id,
            message,
        } => {
            assert_eq!(volume_id, "vol-cache");
            assert_eq!(instance_id, "id");
            assert!(!message.is_empty(), "the provider message was dropped");
        }
        other => return Err(anyhow::anyhow!("unexpected error: {other:?}")),
    }
    assert_eq!(stub.finish().await?.len(), 1);
    Ok(())
}

/// Scenario: detaching a cache volume.
///
/// Invariant: the backend reads the instance to find its current root volume,
/// then patches only index `"0"`, so detaching cannot drop the root volume.
#[rstest]
#[tokio::test]
async fn detach_volume_keeps_only_the_root_index(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let stub = StubApi::start(vec![
        Reply {
            status: 200,
            body: include_str!("fixtures/detach-instance.json").to_owned(),
        },
        patched(),
    ])
    .await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    backend_fixture.detach_volume(&handle, "vol-cache").await?;
    let requests = stub.finish().await?;
    assert_eq!(requests.len(), 2);
    let read = requests.first().context("instance read missing")?;
    assert!(read.starts_with("GET "), "{read}");
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
/// Invariant: the detach stops with `VolumeNotFound` before sending a PATCH,
/// so an unexpected provider shape cannot clear every volume index.
#[rstest]
#[tokio::test]
async fn detach_volume_rejects_an_instance_without_a_root_volume(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let stub = StubApi::start(vec![Reply {
        status: 200,
        body: rootless_instance()?,
    }])
    .await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let error = backend_fixture
        .detach_volume(&handle, "vol-cache")
        .await
        .expect_err("a detached cache volume cannot supply a root volume");
    assert!(
        matches!(error, ScalewayBackendError::VolumeNotFound { ref volume_id, .. } if volume_id == "0"),
        "{error:?}"
    );
    assert_eq!(stub.finish().await?.len(), 1);
    Ok(())
}

/// Removes every volume from the fixture, modelling an instance whose
/// snapshot reports no root volume to preserve.
fn rootless_instance() -> Result<String> {
    let mut instance: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/detach-instance.json"))
            .context("detach fixture is not JSON")?;
    let volumes = instance
        .pointer_mut("/server/volumes")
        .context("detach fixture has no volumes")?
        .as_object_mut()
        .context("detach fixture volumes are not an object")?;
    volumes.clear();
    Ok(serde_json::to_string(&instance)?)
}

/// Scenario: creating a Block Storage volume.
///
/// Invariant: the POST carries the requested name, size, and project, and the
/// provider's identifiers come back as the handle.
#[rstest]
#[tokio::test]
async fn create_volume_posts_the_request_and_returns_the_handle(
    mut backend_fixture: ScalewayBackend,
) -> Result<()> {
    let stub = StubApi::start(vec![created_volume()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let wanted = VolumeRequest::new("cache", 10_000_000_000, "zone", "proj");
    let volume = backend_fixture.create_volume(&wanted).await?;
    assert_eq!(volume.id, "vol-created");
    assert_eq!(volume.zone, "zone");
    let requests = stub.finish().await?;
    assert_eq!(requests.len(), 1);
    let request = requests.first().context("creation request missing")?;
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

/// Scenario: the provider rejects a volume creation.
///
/// Invariant: a non-success response becomes `VolumeCreateFailed`, naming the
/// requested volume so the caller can report which one failed.
#[rstest]
#[tokio::test]
async fn create_volume_reports_a_provider_rejection(
    mut backend_fixture: ScalewayBackend,
) -> Result<()> {
    let stub = StubApi::start(vec![rejected()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let request = VolumeRequest::new("cache", 10_000_000_000, "zone", "proj");
    let error = backend_fixture
        .create_volume(&request)
        .await
        .expect_err("a rejected creation must fail");
    match error {
        ScalewayBackendError::VolumeCreateFailed {
            name,
            zone,
            message,
        } => {
            assert_eq!(name, "cache");
            assert_eq!(zone, "zone");
            assert!(!message.is_empty(), "the provider message was dropped");
        }
        other => return Err(anyhow::anyhow!("unexpected error: {other:?}")),
    }
    assert_eq!(stub.finish().await?.len(), 1);
    Ok(())
}
