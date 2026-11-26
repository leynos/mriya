//! Tests for the dedicated Instances attach and detach endpoints.
//!
//! The payload assertions are the contract: attach names one volume with
//! `volume_type: sbs_volume`, detach names one volume, and neither rebuilds the
//! server's map. Each success is followed by the Block Storage status wait, and
//! a response that violates the map contract is rejected before the wait.

use std::collections::BTreeMap;

use anyhow::Result;
use rstest::rstest;

use super::{
    backend_fixture,
    handle,
    http_stub::{Reply, request_body, request_url, with_stub},
};
use crate::{
    backend::InstanceHandle,
    scaleway::{
        ScalewayBackend,
        ScalewayBackendError,
        volume::{ExpectedVolumeMap, ServerVolumeEntry},
    },
};

/// The cache volume identifier used by the attachment cases.
pub(super) const CACHE_ID: &str = "11111111-1111-1111-1111-111111111111";

/// The pre-attachment map: only the root volume is present.
pub(super) fn expected_map() -> ExpectedVolumeMap {
    BTreeMap::from([(
        String::from("0"),
        ServerVolumeEntry {
            id: String::from("root-id"),
            volume_type: Some(String::from("sbs_volume")),
        },
    )])
}

/// A successful attach response carrying the cache at index "1".
pub(super) fn attached_server() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/attach-server.json").to_owned(),
    }
}

/// A successful detach response keeping only the root volume.
pub(super) fn detached_server() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/detach-server.json").to_owned(),
    }
}

/// The Block Storage status after a successful attachment.
pub(super) fn in_use_volume() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/block-volume-in-use.json").to_owned(),
    }
}

/// The Block Storage status after a successful detachment.
pub(super) fn available_volume() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/block-volume-available.json").to_owned(),
    }
}

/// A structured provider denial shared by the rejection cases.
pub(super) fn rejected() -> Reply {
    Reply {
        status: 400,
        body: String::from(r#"{"type":"invalid_arguments","message":"volume is busy"}"#),
    }
}

/// Builds an attach response with an arbitrary volume map body.
pub(super) fn server_with(volumes: &serde_json::Value) -> Reply {
    Reply {
        status: 200,
        body: serde_json::json!({"server": {"id": "id", "volumes": volumes}}).to_string(),
    }
}

/// Scenario: attaching the cache to a stopped instance.
///
/// Invariant: the POST names the volume and `sbs_volume` exactly, the returned
/// map keeps the root entry, and the backend then waits for `in_use`.
#[rstest]
#[tokio::test]
async fn attach_volume_posts_the_sbs_reference_and_waits(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![attached_server(), in_use_volume()],
        async |backend| {
            backend
                .attach_volume(&handle, CACHE_ID, &expected_map())
                .await
        },
    )
    .await?;
    result?;

    let attach = requests.first().expect("attach request missing");
    assert!(attach.starts_with("POST "), "{attach}");
    assert_eq!(
        request_url(attach)?.path(),
        "/instance/v1/zones/zone/servers/id/attach-volume"
    );
    assert_eq!(
        request_body(attach)?,
        serde_json::json!({"volume_id": CACHE_ID, "volume_type": "sbs_volume"})
    );
    assert!(attach.contains("x-auth-token: dummy"), "{attach}");

    let status_read = requests.get(1).expect("status read missing");
    assert!(status_read.starts_with("GET "), "{status_read}");
    assert_eq!(
        request_url(status_read)?.path(),
        format!("/block/v1/zones/zone/volumes/{CACHE_ID}")
    );
    Ok(())
}

/// Scenario: the provider rejects the attachment.
///
/// Invariant: the error names both resources and carries the provider body,
/// and no status read follows.
#[rstest]
#[tokio::test]
async fn attach_volume_reports_a_provider_rejection(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![rejected()], async |backend| {
        backend
            .attach_volume(&handle, CACHE_ID, &expected_map())
            .await
    })
    .await?;

    assert_eq!(
        result.expect_err("a rejected attach must fail"),
        ScalewayBackendError::VolumeAttachmentFailed {
            volume_id: String::from(CACHE_ID),
            instance_id: String::from("id"),
            message: rejected().body,
        }
    );
    assert_eq!(requests.len(), 1);
    Ok(())
}

/// One unverifiable attach response and the fragment its error must contain.
#[derive(Clone, Copy)]
struct UnverifiedCase {
    /// Volume map returned by a 2xx attach response.
    volumes: &'static str,
    /// Fragment the unverified error must name.
    marker: &'static str,
}

/// Covers every way a 2xx attachment response can violate the map contract.
#[rstest]
#[case::changed_root(UnverifiedCase {
    volumes: r#"{
        "0": {"id": "different-root", "volume_type": "sbs_volume"},
        "1": {"id": "11111111-1111-1111-1111-111111111111", "volume_type": "sbs_volume"}
    }"#,
    marker: "changed",
})]
#[case::missing_cache(UnverifiedCase {
    volumes: r#"{"0": {"id": "root-id", "volume_type": "sbs_volume"}}"#,
    marker: "absent",
})]
#[case::wrong_type(UnverifiedCase {
    volumes: r#"{
        "0": {"id": "root-id", "volume_type": "sbs_volume"},
        "1": {"id": "11111111-1111-1111-1111-111111111111", "volume_type": "l_ssd"}
    }"#,
    marker: "l_ssd",
})]
#[tokio::test]
async fn attach_volume_rejects_an_unverifiable_map(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
    #[case] case: UnverifiedCase,
) -> Result<()> {
    let volumes: serde_json::Value =
        serde_json::from_str(case.volumes).expect("case volumes are JSON");
    let (result, requests) = with_stub(
        backend_fixture,
        vec![server_with(&volumes)],
        async |backend| {
            backend
                .attach_volume(&handle, CACHE_ID, &expected_map())
                .await
        },
    )
    .await?;

    let error = result.expect_err("an unverifiable map must fail the attach");
    let rendered = format!("{error:?}");
    assert!(
        matches!(
            error,
            ScalewayBackendError::VolumeAttachmentUnverified { ref message, .. }
                if message.contains(case.marker)
        ),
        "expected `{}` in {rendered}",
        case.marker
    );
    assert_eq!(requests.len(), 1, "no status read may follow a failure");
    Ok(())
}

/// Scenario: the cache was attached at the root index.
///
/// Invariant: index "0" is refused even when the identifier and type match.
#[rstest]
#[tokio::test]
async fn attach_volume_rejects_the_root_index(backend_fixture: ScalewayBackend) -> Result<()> {
    let handle = InstanceHandle {
        id: String::from("id"),
        zone: String::from("zone"),
    };
    let expected = BTreeMap::from([(
        String::from("0"),
        ServerVolumeEntry {
            id: String::from("root-id"),
            volume_type: Some(String::from("sbs_volume")),
        },
    )]);
    let reply = server_with(&serde_json::json!({
        "0": {"id": CACHE_ID, "volume_type": "sbs_volume"},
    }));
    let (result, _) = with_stub(backend_fixture, vec![reply], async |backend| {
        backend.attach_volume(&handle, CACHE_ID, &expected).await
    })
    .await?;

    assert!(matches!(
        result.expect_err("the root index must be refused"),
        ScalewayBackendError::VolumeAttachmentUnverified { .. }
    ));
    Ok(())
}
