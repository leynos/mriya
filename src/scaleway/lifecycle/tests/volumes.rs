//! Tests through the real Block Storage volume wrappers and a loopback API.
//!
//! Each case points the backend at [`StubApi`] and asserts the request the
//! provider would receive. The payload assertions are the contract: creation
//! names the project, the IOPS tier and a decimal byte size, never a retired
//! `volume_type`; the tier gate suppresses the POST when the zone lacks the
//! tier; and every request authenticates.

use anyhow::{Context, Result};
use rstest::rstest;

use super::{
    backend_fixture,
    http_stub::{Reply, request_body, request_url, with_stub},
};
use crate::{
    scaleway::{ScalewayBackend, ScalewayBackendError},
    volume::{VolumeHandle, VolumeRequest, VolumeState, VolumeStatus, VolumeTier},
};

/// A provider reply carrying a volume fixture.
pub(super) fn volume_reply(fixture: &str) -> Reply {
    Reply {
        status: 200,
        body: fixture.to_owned(),
    }
}

/// The zone's volume-type listing, including both SBS tiers.
pub(super) fn volume_types() -> Reply { volume_reply(include_str!("fixtures/volume-types.json")) }

/// A zone listing that offers only the 5K tier.
pub(super) fn volume_types_5k_only() -> Reply {
    volume_reply(include_str!("fixtures/volume-types-5k-only.json"))
}

/// A successful create response for a volume in the fixture zone.
pub(super) fn created_volume() -> Reply {
    volume_reply(include_str!("fixtures/block-volume-available.json"))
}

/// A structured provider denial shared by the rejection cases.
pub(super) fn rejected() -> Reply {
    Reply {
        status: 404,
        body: String::from(r#"{"type":"not_found","message":"no such volume"}"#),
    }
}

/// Builds a volume state directly for the polling-loop cases.
pub(super) fn state(status: &str) -> VolumeState {
    VolumeState {
        id: String::from("11111111-1111-1111-1111-111111111111"),
        zone: String::from("zone"),
        project_id: String::from("proj"),
        status: VolumeStatus::from_provider(status),
        volume_type: String::from("sbs_5k"),
        storage_class: Some(String::from("sbs")),
        size_bytes: 20_000_000_000,
        references: Vec::new(),
    }
}

/// Parses one fixture into the orchestrator-facing volume state.
pub(super) fn fixture_state(fixture: &str) -> Result<VolumeState> {
    let volume: crate::scaleway::types::SbsVolume =
        serde_json::from_str(fixture).context("volume fixture is not JSON")?;
    Ok(volume.into_state())
}

/// Scenario: creating a Block Storage volume.
///
/// Invariant: the tier gate reads the zone's volume types, then the POST
/// carries the name, project, IOPS and decimal size with the cache tags, and
/// never the retired `volume_type` field.
#[rstest]
#[tokio::test]
async fn create_volume_gates_on_the_tier_then_posts_the_sbs_body(
    backend_fixture: ScalewayBackend,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![volume_types(), created_volume()],
        async |backend| {
            let request = VolumeRequest::new("cache", 20_000_000_000, "zone", "proj")
                .with_tier(VolumeTier::Sbs15k);
            backend.create_volume(&request).await
        },
    )
    .await?;

    let handle: VolumeHandle = result?;
    assert_eq!(handle.id, "11111111-1111-1111-1111-111111111111");
    assert_eq!(handle.zone, "zone");

    let listing = requests.first().context("tier listing missing")?;
    assert!(listing.starts_with("GET "), "{listing}");
    assert_eq!(
        request_url(listing)?.path(),
        "/block/v1/zones/zone/volume-types"
    );

    let create = requests.get(1).context("create request missing")?;
    assert!(create.starts_with("POST "), "{create}");
    assert_eq!(request_url(create)?.path(), "/block/v1/zones/zone/volumes");
    assert!(
        create.contains("x-auth-token: dummy"),
        "create must carry the configured token: {create}"
    );
    assert_eq!(
        request_body(create)?,
        serde_json::json!({
            "name": "cache",
            "project_id": "proj",
            "perf_iops": 15_000,
            "from_empty": {"size": 20_000_000_000u64},
            "tags": ["mriya", "cache"],
        })
    );
    Ok(())
}

/// Scenario: the zone does not stock the requested tier.
///
/// Invariant: the create stops after the listing and sends no POST, so an
/// unsupported tier cannot allocate a volume.
#[rstest]
#[tokio::test]
async fn create_volume_rejects_a_tier_absent_from_the_zone(
    backend_fixture: ScalewayBackend,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![volume_types_5k_only()],
        async |backend| {
            let request = VolumeRequest::new("cache", 20_000_000_000, "zone", "proj")
                .with_tier(VolumeTier::Sbs15k);
            backend.create_volume(&request).await
        },
    )
    .await?;

    let error = result.expect_err("an unavailable tier must not create a volume");
    assert_eq!(
        error,
        ScalewayBackendError::VolumeTierUnavailable {
            tier: VolumeTier::Sbs15k,
            zone: String::from("zone"),
        }
    );
    assert_eq!(requests.len(), 1, "no POST may follow a failed gate");
    Ok(())
}

/// Scenario: the provider rejects a create.
///
/// Invariant: the error names the volume and zone and carries the provider's
/// own message.
#[rstest]
#[tokio::test]
async fn create_volume_reports_the_provider_rejection(
    backend_fixture: ScalewayBackend,
) -> Result<()> {
    let (result, _) = with_stub(
        backend_fixture,
        vec![volume_types(), rejected()],
        async |backend| {
            backend
                .create_volume(&VolumeRequest::new("cache", 20_000_000_000, "zone", "proj"))
                .await
        },
    )
    .await?;

    assert_eq!(
        result.expect_err("a rejected create must fail"),
        ScalewayBackendError::VolumeCreateFailed {
            name: String::from("cache"),
            zone: String::from("zone"),
            message: rejected().body,
        }
    );
    Ok(())
}

/// Scenario: a request below the provider minimum.
///
/// Invariant: validation runs before the tier gate, so no request is sent.
#[rstest]
#[tokio::test]
async fn create_volume_rejects_a_size_below_the_minimum(
    backend_fixture: ScalewayBackend,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![], async |backend| {
        backend
            .create_volume(&VolumeRequest::new("cache", 1_000_000_000, "zone", "proj"))
            .await
    })
    .await?;

    assert!(matches!(
        result.expect_err("an undersized volume must fail"),
        ScalewayBackendError::Validation(_)
    ));
    assert!(requests.is_empty(), "{requests:?}");
    Ok(())
}
