//! Loopback tests for volume lookup, readiness polling and deletion.
//!
//! The 404 zone mapping, the terminal `error` status, the timeout and the
//! idempotent delete are all provider contracts; these tests assert the exact
//! requests and error values.

use std::{collections::VecDeque, time::Duration};

use anyhow::Result;
use rstest::rstest;

use super::{
    backend_fixture,
    http_stub::{Reply, only_request, request_url, with_stub},
    volumes::{fixture_state, rejected, state, volume_reply},
};
use crate::{
    scaleway::{
        ScalewayBackend,
        ScalewayBackendError,
        lifecycle::block_volume::{VolumePollSettings, poll_for_volume_status},
    },
    volume::{VolumeHandle, VolumeStatus},
};

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
/// recognized, so preflight can refuse the volume before any server exists.
#[rstest]
fn legacy_fixture_is_classified_as_legacy() -> Result<()> {
    let state = fixture_state(include_str!("fixtures/block-volume-legacy.json"))?;
    anyhow::ensure!(
        state.is_legacy(),
        "legacy fixture was not classified as legacy"
    );
    anyhow::ensure!(
        state.status == VolumeStatus::Available,
        "legacy fixture should be available, got {}",
        state.status
    );
    Ok(())
}
