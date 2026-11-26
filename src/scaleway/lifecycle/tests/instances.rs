//! Tests through the real instance creation wrapper and a loopback Scaleway
//! API.
//!
//! Creation must reach the substitutable API base rather than the SDK's
//! hard-coded production host, and the server it posts must start stopped so
//! cloud-init user-data is in place before first boot. A hard-coded base or a
//! dropped `stopped` flag therefore fails here instead of in a live account.

use anyhow::{Context, Result};
use rstest::rstest;

use super::{
    backend_fixture,
    base_request,
    http_stub::{Reply, only_request, request_body, request_url, with_stub},
};
use crate::{
    backend::InstanceRequest,
    boot_volume::{BootVolumePlan, BootVolumeType, CompatibilityDefault},
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

/// A successful creation, reusing the single-server fixture the detach path
/// also decodes.
fn created_instance() -> Reply {
    Reply {
        status: 201,
        body: include_str!("fixtures/detach-instance.json").to_owned(),
    }
}

/// A structured provider rejection that does not identify the instance type.
fn rejected() -> Reply {
    Reply {
        status: 404,
        body: String::from(r#"{"type":"denied","message":"no such image"}"#),
    }
}

/// A 20 GB local plan, used wherever a create needs a boot specification.
fn local_plan() -> BootVolumePlan {
    BootVolumePlan::new(
        BootVolumeType::Local,
        20_000_000_000,
        CompatibilityDefault::default(),
    )
}

/// Scenario: creating an instance before its cache volume is attached.
///
/// Invariant: the POST targets the configured zone on the substitutable base
/// and sends a stopped server tagged as ephemeral, and the provider's server
/// is returned to the caller.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_posts_a_stopped_server(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let (result, requests) =
        with_stub(backend_fixture, vec![created_instance()], async |backend| {
            backend
                .create_instance_stopped(&base_request, "img-test", &local_plan())
                .await
        })
        .await?;
    let server = result?;
    assert_eq!(server.id, "id");
    let request = only_request(&requests)?;
    assert!(request.starts_with("POST "), "{request}");
    assert_eq!(
        request_url(request)?.path(),
        "/instance/v1/zones/zone/servers"
    );
    let body = request_body(request)?;
    let instance_name = body
        .as_object()
        .context("creation body is not an object")?
        .get("name")
        .context("creation body has no name")?;
    assert!(
        instance_name
            .as_str()
            .is_some_and(|name| name.starts_with("mriya-")),
        "unexpected instance name: {instance_name}"
    );
    // The name is random per call, so the remaining payload is compared as a
    // whole. That also proves the optional fields stay absent and that the
    // boot volume is present with exactly the plan's type and size.
    assert_eq!(
        without_name(&body)?,
        serde_json::json!({
            "commercial_type": "type",
            "image": "img-test",
            "project": "proj",
            "routed_ip_enabled": true,
            "dynamic_ip_required": true,
            "tags": ["mriya", "ephemeral"],
            "stopped": true,
            "volumes": {
                "0": {
                    "volume_type": "l_ssd",
                    "size": 20_000_000_000u64,
                    "boot": true,
                }
            },
        })
    );
    Ok(())
}

/// Scenario: creating an instance with a Block Storage boot volume.
///
/// Invariant: the `volumes["0"]` entry carries `sbs_volume`, the planned byte
/// size, and the boot flag, so the provider creates the disk that was priced
/// rather than one of its defaults.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_sends_an_sbs_boot_volume(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let plan = BootVolumePlan::new(
        BootVolumeType::Sbs,
        30_000_000_000,
        CompatibilityDefault::default(),
    );
    let (result, requests) =
        with_stub(backend_fixture, vec![created_instance()], async |backend| {
            backend
                .create_instance_stopped(&base_request, "img-test", &plan)
                .await
        })
        .await?;
    result?;
    let body = request_body(only_request(&requests)?)?;
    assert_eq!(
        body.pointer("/volumes/0").context("boot entry missing")?,
        &serde_json::json!({
            "volume_type": "sbs_volume",
            "size": 30_000_000_000u64,
            "boot": true,
        })
    );
    Ok(())
}

/// Scenario: the create request must not smuggle the cache volume into the
/// boot entry.
///
/// Invariant: a request carrying a cache volume ID still sends only the boot
/// volume at index `"0"`; the cache is attached later, by the PATCH, and never
/// appears as `size` or `id` of the boot entry.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_keeps_the_cache_volume_out_of_the_boot_entry(
    backend_fixture: ScalewayBackend,
    mut base_request: InstanceRequest,
) -> Result<()> {
    base_request.volume_id = Some(String::from("vol-cache"));
    let (result, requests) =
        with_stub(backend_fixture, vec![created_instance()], async |backend| {
            backend
                .create_instance_stopped(&base_request, "img-test", &local_plan())
                .await
        })
        .await?;
    result?;
    let body = request_body(only_request(&requests)?)?;
    let boot_entry = body
        .pointer("/volumes/0")
        .context("boot entry missing")?
        .as_object()
        .context("boot entry is not an object")?;
    assert!(
        !boot_entry.contains_key("id"),
        "the boot entry must not carry a volume ID: {boot_entry:?}"
    );
    assert!(
        !body.to_string().contains("vol-cache"),
        "the cache volume must not appear in the create body: {body}"
    );
    Ok(())
}

/// Returns the creation payload minus its random `name` field.
fn without_name(body: &serde_json::Value) -> Result<serde_json::Value> {
    let mut fields = body
        .as_object()
        .context("creation body is not an object")?
        .clone();
    fields.remove("name");
    Ok(serde_json::Value::Object(fields))
}

/// Scenario: the provider rejects a creation that is not a type mismatch.
///
/// Invariant: the raw provider message survives as a `Provider` error rather
/// than being misreported as an unavailable instance type.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_preserves_a_provider_rejection(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![rejected()], async |backend| {
        backend
            .create_instance_stopped(&base_request, "img-test", &local_plan())
            .await
    })
    .await?;
    let error = result.expect_err("a rejected creation must fail");
    assert!(
        matches!(error, ScalewayBackendError::Provider { ref message } if message.contains("no such image")),
        "{error:?}"
    );
    assert_eq!(requests.len(), 1);
    Ok(())
}

/// Scenario: the provider answers a create with a Block Storage root volume.
///
/// Invariant: the response decodes even though the volume reports
/// `"size": null`, and the boot entry is found by its flag. The SDK's instance
/// type requires a size on every volume, which is why creation uses a local
/// minimal model.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_decodes_an_sbs_create_response(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let sbs_response = Reply {
        status: 201,
        body: include_str!("fixtures/create-server-sbs.json").to_owned(),
    };
    let plan = BootVolumePlan::new(
        BootVolumeType::Sbs,
        20_000_000_000,
        CompatibilityDefault::default(),
    );

    let (result, _) = with_stub(backend_fixture, vec![sbs_response], async |backend| {
        backend
            .create_instance_stopped(&base_request, "img-test", &plan)
            .await
    })
    .await?;

    let server = result?;
    let (index, volume) = server
        .boot_volume()
        .context("the create response should carry a boot volume")?;
    assert_eq!(index, "0");
    assert_eq!(volume.id, "vol-root");
    assert_eq!(volume.volume_type.as_deref(), Some("sbs_volume"));
    assert_eq!(volume.size, None, "an SBS entry reports no size here");
    Ok(())
}
