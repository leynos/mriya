//! Tests through the real Block Storage volume wrappers and a loopback API.
//!
//! Each case points the backend at [`StubApi`] and asserts the request the
//! provider would receive. The payload assertions are the contract: creation
//! names the project, the IOPS tier and a decimal byte size, never a retired
//! `volume_type`; the tier gate suppresses the POST when the zone lacks the
//! tier; and every request authenticates.

use std::{collections::VecDeque, time::Duration};

use anyhow::{Context, Result};
use rstest::rstest;

use super::{
    backend_fixture,
    http_stub::{Reply, only_request, request_body, request_url, with_stub},
};
use crate::{
    scaleway::{
        ScalewayBackend,
        ScalewayBackendError,
        lifecycle::block_volume::{VolumePollSettings, poll_for_volume_status},
    },
    volume::{VolumeHandle, VolumeRequest, VolumeState, VolumeStatus, VolumeTier},
};

/// A provider reply carrying a volume fixture.
fn volume_reply(fixture: &str) -> Reply {
    Reply {
        status: 200,
        body: fixture.to_owned(),
    }
}

/// The zone's volume-type listing, including both SBS tiers.
fn volume_types() -> Reply { volume_reply(include_str!("fixtures/volume-types.json")) }

/// A zone listing that offers only the 5K tier.
fn volume_types_5k_only() -> Reply {
    volume_reply(include_str!("fixtures/volume-types-5k-only.json"))
}

/// A successful create response for a volume in the fixture zone.
fn created_volume() -> Reply { volume_reply(include_str!("fixtures/block-volume-available.json")) }

/// A structured provider denial shared by the rejection cases.
fn rejected() -> Reply {
    Reply {
        status: 404,
        body: String::from(r#"{"type":"not_found","message":"no such volume"}"#),
    }
}

/// Builds a volume state directly for the polling-loop cases.
fn state(status: &str) -> VolumeState {
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
fn fixture_state(fixture: &str) -> Result<VolumeState> {
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

/// Scenario: reading a volume.
///
/// Invariant: a success body is the unwrapped `Volume`, and its fields are
/// mapped into the orchestrator-facing state.
#[rstest]
#[tokio::test]
async fn get_volume_parses_the_unwrapped_object(backend_fixture: ScalewayBackend) -> Result<()> {
    let body = include_str!("fixtures/block-volume-in-use.json");
    let (result, requests) =
        with_stub(backend_fixture, vec![volume_reply(body)], async |backend| {
            backend
                .get_volume("zone", "11111111-1111-1111-1111-111111111111")
                .await
        })
        .await?;

    let state = result?;
    assert_eq!(state.status, VolumeStatus::InUse);
    assert_eq!(state.volume_type, "sbs_5k");
    assert_eq!(state.storage_class.as_deref(), Some("sbs"));
    assert_eq!(state.references.len(), 1);
    let request = only_request(&requests)?;
    assert_eq!(
        request_url(request)?.path(),
        "/block/v1/zones/zone/volumes/11111111-1111-1111-1111-111111111111"
    );
    assert!(request.contains("x-auth-token: dummy"), "{request}");
    Ok(())
}

/// Scenario: a lookup in the wrong zone.
///
/// Invariant: the 404 becomes `VolumeNotInZone`, whose message states the
/// same-zone requirement and names the zone that was searched.
#[rstest]
#[tokio::test]
async fn get_volume_maps_a_404_to_the_zone_error(backend_fixture: ScalewayBackend) -> Result<()> {
    let (result, _) = with_stub(backend_fixture, vec![rejected()], async |backend| {
        backend
            .get_volume("zone", "11111111-1111-1111-1111-111111111111")
            .await
    })
    .await?;

    let error = result.expect_err("a missing volume must fail");
    assert_eq!(
        error,
        ScalewayBackendError::VolumeNotInZone {
            volume_id: String::from("11111111-1111-1111-1111-111111111111"),
            zone: String::from("zone"),
        }
    );
    assert!(error.to_string().contains("same Availability Zone"));
    Ok(())
}

/// Scenario: creation reports `creating` before `available`.
///
/// Invariant: the poller keeps reading until the expected status appears and
/// returns that final state.
#[rstest]
#[tokio::test]
async fn wait_for_volume_status_returns_the_expected_state() -> Result<()> {
    let mut observations = VecDeque::from(vec![state("creating"), state("available")]);
    let result = poll_for_volume_status(
        "11111111-1111-1111-1111-111111111111",
        &VolumeStatus::Available,
        VolumePollSettings {
            poll_interval: Duration::from_millis(1),
            wait_timeout: Duration::from_millis(50),
        },
        || {
            let next = observations
                .pop_front()
                .expect("polling exceeded the scripted states");
            async move { Ok(next) }
        },
    )
    .await;

    assert_eq!(result?.status, VolumeStatus::Available);
    Ok(())
}

/// Scenario: the volume enters the provider's `error` state.
///
/// Invariant: polling fails immediately instead of waiting for the deadline.
#[rstest]
#[tokio::test]
async fn wait_for_volume_status_fails_on_the_error_state() -> Result<()> {
    let mut observations = VecDeque::from(vec![state("creating"), state("error")]);
    let result = poll_for_volume_status(
        "11111111-1111-1111-1111-111111111111",
        &VolumeStatus::Available,
        VolumePollSettings {
            poll_interval: Duration::from_millis(1),
            wait_timeout: Duration::from_secs(60),
        },
        || {
            let next = observations
                .pop_front()
                .expect("polling exceeded the scripted states");
            async move { Ok(next) }
        },
    )
    .await;

    assert_eq!(
        result.expect_err("an error status must fail"),
        ScalewayBackendError::VolumeUnavailable {
            volume_id: String::from("11111111-1111-1111-1111-111111111111"),
            status: String::from("error"),
        }
    );
    Ok(())
}

/// Scenario: the volume never reaches the expected status.
///
/// Invariant: the deadline produces a typed timeout naming the expected and
/// last observed statuses.
#[rstest]
#[tokio::test]
async fn wait_for_volume_status_reports_a_timeout() -> Result<()> {
    let result = poll_for_volume_status(
        "11111111-1111-1111-1111-111111111111",
        &VolumeStatus::Available,
        VolumePollSettings {
            poll_interval: Duration::from_millis(1),
            wait_timeout: Duration::ZERO,
        },
        || async { Ok(state("creating")) },
    )
    .await;

    assert_eq!(
        result.expect_err("an unreachable status must time out"),
        ScalewayBackendError::VolumeStatusTimeout {
            volume_id: String::from("11111111-1111-1111-1111-111111111111"),
            expected: String::from("available"),
            last_status: String::from("creating"),
        }
    );
    Ok(())
}

/// Scenario: deleting an owned volume.
///
/// Invariant: a 204 and a 404 both count as success, so a retried teardown is
/// idempotent; any other rejection names the volume.
#[rstest]
#[case::deleted(204, "")]
#[case::already_gone(404, r#"{"type":"not_found","message":"gone"}"#)]
#[tokio::test]
async fn delete_volume_accepts_success_and_not_found(
    backend_fixture: ScalewayBackend,
    #[case] status: u16,
    #[case] body: &str,
) -> Result<()> {
    let handle = VolumeHandle {
        id: String::from("11111111-1111-1111-1111-111111111111"),
        zone: String::from("zone"),
    };
    let reply = Reply {
        status,
        body: body.to_owned(),
    };
    let (result, requests) = with_stub(backend_fixture, vec![reply], async |backend| {
        backend.delete_volume(&handle).await
    })
    .await?;

    result?;
    let request = only_request(&requests)?;
    assert!(request.starts_with("DELETE "), "{request}");
    assert_eq!(
        request_url(request)?.path(),
        "/block/v1/zones/zone/volumes/11111111-1111-1111-1111-111111111111"
    );
    Ok(())
}

/// Scenario: the provider rejects a deletion.
///
/// Invariant: the failure carries the provider's message so an operator can
/// tell an attached volume from a permission problem.
#[rstest]
#[tokio::test]
async fn delete_volume_reports_a_rejection(backend_fixture: ScalewayBackend) -> Result<()> {
    let handle = VolumeHandle {
        id: String::from("11111111-1111-1111-1111-111111111111"),
        zone: String::from("zone"),
    };
    let reply = Reply {
        status: 400,
        body: String::from(r#"{"message":"volume is in use"}"#),
    };
    let (result, _) = with_stub(backend_fixture, vec![reply], async |backend| {
        backend.delete_volume(&handle).await
    })
    .await?;

    assert_eq!(
        result.expect_err("a rejected delete must fail"),
        ScalewayBackendError::VolumeDeleteFailed {
            volume_id: String::from("11111111-1111-1111-1111-111111111111"),
            message: String::from(r#"{"message":"volume is in use"}"#),
        }
    );
    Ok(())
}

/// Scenario: classifying a legacy volume fixture.
///
/// Invariant: the Block Storage `b_ssd` type and `bssd` class are both
/// recognised, so preflight can refuse the volume before any server exists.
#[rstest]
fn legacy_fixture_is_classified_as_legacy() -> Result<()> {
    let state = fixture_state(include_str!("fixtures/block-volume-legacy.json"))?;
    assert!(state.is_legacy());
    assert_eq!(state.status, VolumeStatus::Available);
    Ok(())
}
