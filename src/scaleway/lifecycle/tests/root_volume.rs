//! Tests through the root-volume capture and verification boundary.
//!
//! The contract under test is what the account is actually paying for. A local
//! root reports its own size in the create response; a Block Storage root is
//! read back from the Block Storage API. A disagreement with the plan must
//! fail, so a provider substitution cannot silently change the disk.

use anyhow::{Context, Result};
use rstest::rstest;

use super::{
    backend_fixture,
    http_stub::{Reply, request_url, with_stub},
    is_differing_mismatch,
};
use crate::{
    boot_volume::{
        BootVolumeMismatch,
        BootVolumePlan,
        BootVolumeType,
        CompatibilityDefault,
        RootVolume,
    },
    scaleway::{ScalewayBackend, ScalewayBackendError, lifecycle::create::CreatedServer},
};

/// A 20 GB local plan, matching `local_server`.
fn local_plan() -> BootVolumePlan {
    BootVolumePlan::new(
        BootVolumeType::Local,
        20_000_000_000,
        CompatibilityDefault::default(),
    )
}

/// A 20 GB Block Storage plan, matching `sbs_server` plus its block read.
fn sbs_plan() -> BootVolumePlan {
    BootVolumePlan::new(
        BootVolumeType::Sbs,
        20_000_000_000,
        CompatibilityDefault::default(),
    )
}

/// Builds a created-server response whose boot entry is described by `json`.
fn server(body: serde_json::Value) -> Result<CreatedServer> {
    serde_json::from_value(body).context("created-server fixture should decode")
}

/// A server whose root volume is a 20 GB local disk.
pub(super) fn local_server() -> Result<CreatedServer> {
    server(serde_json::json!({
        "id": "srv-1",
        "state": "stopped",
        "volumes": {
            "0": {
                "id": "vol-root",
                "size": 20_000_000_000u64,
                "volume_type": "l_ssd",
                "boot": true,
            }
        }
    }))
}

/// A server whose root volume is a Block Storage disk.
///
/// The fixture reports `"size": null` on purpose: that is exactly why the
/// Block Storage read exists, and it is why the create response is decoded
/// with a local model rather than the SDK's instance type.
fn sbs_server() -> Result<CreatedServer> {
    // The fixture is a complete provider response, so it is unwrapped the way
    // creation unwraps it. Keeping it wrapped lets the same file serve the
    // create-payload test, which posts it as a raw body.
    #[derive(serde::Deserialize)]
    struct Wrapper {
        server: CreatedServer,
    }
    let wrapped: Wrapper = serde_json::from_str(include_str!("fixtures/create-server-sbs.json"))
        .context("SBS create fixture should decode")?;
    Ok(wrapped.server)
}

/// A Block Storage read reporting a 20 GB volume.
fn block_volume_reply() -> Reply {
    Reply {
        status: 200,
        body: String::from(r#"{"id":"vol-root","type":"sbs_5k","size":20000000000}"#),
    }
}

/// Scenario: a local root that matches the plan.
///
/// Invariant: the allocation pairs the plan with the reported identifier,
/// type, and size, and no Block Storage request is made.
#[rstest]
#[tokio::test]
async fn local_root_is_captured_from_the_create_response(
    backend_fixture: ScalewayBackend,
) -> Result<()> {
    let server = local_server()?;
    let (result, requests) = with_stub(backend_fixture, vec![], async |backend| {
        backend
            .verify_root_volume(&server, "zone", &local_plan())
            .await
    })
    .await?;

    let allocation = result?;
    assert_eq!(allocation.root.id, "vol-root");
    assert_eq!(allocation.root.volume_type, BootVolumeType::Local);
    assert_eq!(allocation.root.size_bytes, 20_000_000_000);
    assert!(
        requests.is_empty(),
        "a local root must not need a Block Storage read: {requests:?}"
    );
    Ok(())
}

/// Scenario: a Block Storage root whose size only the Block Storage API knows.
///
/// Invariant: the size is read from `/block/v1/zones/{zone}/volumes/{id}` and
/// recorded, and the request carries the configured token.
#[rstest]
#[tokio::test]
async fn sbs_root_is_read_from_the_block_api(backend_fixture: ScalewayBackend) -> Result<()> {
    let server = sbs_server()?;
    let (result, requests) = with_stub(
        backend_fixture,
        vec![block_volume_reply()],
        async |backend| {
            backend
                .verify_root_volume(&server, "zone", &sbs_plan())
                .await
        },
    )
    .await?;

    let allocation = result?;
    assert_eq!(allocation.root.id, "vol-root");
    assert_eq!(allocation.root.volume_type, BootVolumeType::Sbs);
    assert_eq!(allocation.root.size_bytes, 20_000_000_000);
    let request = super::http_stub::only_request(&requests)?;
    assert!(request.starts_with("GET "), "{request}");
    assert_eq!(
        request_url(request)?.path(),
        "/block/v1/zones/zone/volumes/vol-root"
    );
    assert!(
        request.contains("x-auth-token: dummy"),
        "the read must authenticate: {request}"
    );
    Ok(())
}

/// Scenario: the provider reports a different size from the plan.
///
/// Invariant: the mismatch fails naming both sizes, so a silently resized disk
/// cannot be billed as the requested one.
#[rstest]
#[tokio::test]
async fn a_differing_local_size_is_rejected(backend_fixture: ScalewayBackend) -> Result<()> {
    let server = server(serde_json::json!({
        "id": "srv-1",
        "state": "stopped",
        "volumes": {
            "0": {
                "id": "vol-root",
                "size": 10_000_000_000u64,
                "volume_type": "l_ssd",
                "boot": true,
            }
        }
    }))?;
    let (result, _) = with_stub(backend_fixture, vec![], async |backend| {
        backend
            .verify_root_volume(&server, "zone", &local_plan())
            .await
    })
    .await?;

    let error = result.expect_err("a smaller root than planned must fail");
    assert!(is_differing_mismatch(&error), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("20 GB"), "{message}");
    assert!(message.contains("10 GB"), "{message}");
    Ok(())
}

/// Scenario: the provider substitutes a different storage type.
///
/// Invariant: an `sbs_volume` answering a local plan fails rather than being
/// accepted as "close enough".
#[rstest]
#[tokio::test]
async fn a_differing_type_is_rejected(backend_fixture: ScalewayBackend) -> Result<()> {
    let server = sbs_server()?;
    let (result, _) = with_stub(
        backend_fixture,
        vec![block_volume_reply()],
        async |backend| {
            backend
                .verify_root_volume(&server, "zone", &local_plan())
                .await
        },
    )
    .await?;

    let error = result.expect_err("an SBS root answering a local plan must fail");
    let message = error.to_string();
    assert!(message.contains("sbs_volume"), "{message}");
    assert!(message.contains("l_ssd"), "{message}");
    Ok(())
}

/// Scenario: the create response reports no bootable volume.
///
/// Invariant: the capture fails as missing rather than guessing at index "0"
/// when the response carries no volumes at all.
#[rstest]
#[tokio::test]
async fn a_missing_root_is_reported(backend_fixture: ScalewayBackend) -> Result<()> {
    let server = server(serde_json::json!({"id": "srv-1", "state": "stopped"}))?;
    let (result, _) = with_stub(backend_fixture, vec![], async |backend| {
        backend
            .verify_root_volume(&server, "zone", &local_plan())
            .await
    })
    .await?;

    let error = result.expect_err("a response with no volumes cannot be verified");
    assert!(
        matches!(
            error,
            ScalewayBackendError::BootVolumeMismatch(ref mismatch)
                if matches!(**mismatch, BootVolumeMismatch::Missing { ref instance_id } if instance_id == "srv-1")
        ),
        "{error:?}"
    );
    Ok(())
}

/// Scenario: the Block Storage read is rejected.
///
/// Invariant: the failure names the volume and zone, so an operator can tell a
/// provider permission problem from a missing disk.
#[rstest]
#[tokio::test]
async fn a_rejected_block_read_is_reported(backend_fixture: ScalewayBackend) -> Result<()> {
    let server = sbs_server()?;
    let rejection = Reply {
        status: 404,
        body: String::from(r#"{"message":"no such volume"}"#),
    };
    let (result, _) = with_stub(backend_fixture, vec![rejection], async |backend| {
        backend
            .verify_root_volume(&server, "zone", &sbs_plan())
            .await
    })
    .await?;

    let error = result.expect_err("a rejected block read must fail");
    assert!(
        matches!(
            error,
            ScalewayBackendError::BlockVolumeReadFailed {
                ref volume_id,
                ref zone,
                ..
            } if volume_id == "vol-root" && zone == "zone"
        ),
        "{error:?}"
    );
    Ok(())
}

/// The boot entry is chosen by its flag rather than by index when the provider
/// says which entry boots.
#[rstest]
#[tokio::test]
async fn the_boot_flag_selects_the_root_entry(backend_fixture: ScalewayBackend) -> Result<()> {
    let server = server(serde_json::json!({
        "id": "srv-1",
        "state": "stopped",
        "volumes": {
            "0": {
                "id": "vol-something-else",
                "size": 10_000_000_000u64,
                "volume_type": "l_ssd",
                "boot": false,
            },
            "1": {
                "id": "vol-root",
                "size": 20_000_000_000u64,
                "volume_type": "l_ssd",
                "boot": true,
            }
        }
    }))?;
    let (result, _) = with_stub(backend_fixture, vec![], async |backend| {
        backend
            .verify_root_volume(&server, "zone", &local_plan())
            .await
    })
    .await?;

    let allocation = result?;
    assert_eq!(allocation.root.id, "vol-root");
    Ok(())
}

/// The reported root round-trips into a `RootVolume` a caller can hold.
#[test]
fn root_volume_carries_the_identifier_type_and_size() {
    let root = RootVolume {
        id: String::from("vol-root"),
        volume_type: BootVolumeType::Sbs,
        size_bytes: 20_000_000_000,
    };
    assert_eq!(root.id, "vol-root");
    assert_eq!(root.volume_type.provider_id(), "sbs_volume");
    assert_eq!(root.size_bytes, 20_000_000_000);
}
