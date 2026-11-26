//! Loopback tests for orchestrated creation with a cache volume.
//!
//! Creation validates the configured volume before the server POST, attaches
//! it through the dedicated endpoint, and powers the server on only after the
//! attachment is confirmed. Any post-POST failure rolls the server back; a
//! rollback that cannot detach the cache retains the server instead of letting
//! the provider delete the cache with it.

use anyhow::Result;
use rstest::rstest;

use super::{
    backend_fixture,
    base_request,
    http_stub::{Reply, request_body, request_url, with_stub},
};
use crate::{backend::InstanceRequest, scaleway::ScalewayBackend};

/// Cache volume identifier used by the creation cases.
pub(super) const CACHE_ID: &str = "11111111-1111-1111-1111-111111111111";

/// A successful server creation carrying one root volume.
pub(super) fn created_server() -> Reply {
    Reply {
        status: 201,
        body: include_str!("fixtures/create-instance.json").to_owned(),
    }
}

/// The Block Storage status before attachment.
pub(super) fn available_volume() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/block-volume-available.json").to_owned(),
    }
}

/// The Block Storage status after attachment.
pub(super) fn in_use_volume() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/block-volume-in-use.json").to_owned(),
    }
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

/// A successful server action.
pub(super) fn action_ok() -> Reply {
    Reply {
        status: 200,
        body: String::from(r#"{"task":{}}"#),
    }
}

/// A rejected power-on action.
pub(super) fn action_rejected() -> Reply {
    Reply {
        status: 400,
        body: String::from(r#"{"type":"invalid_arguments","message":"poweron rejected"}"#),
    }
}

/// A rejected attach request.
pub(super) fn attach_rejected() -> Reply {
    Reply {
        status: 400,
        body: String::from(r#"{"type":"invalid_arguments","message":"volume is busy"}"#),
    }
}

/// A rejected detach request.
pub(super) fn detach_rejected() -> Reply {
    Reply {
        status: 400,
        body: String::from(r#"{"type":"invalid_arguments","message":"volume is busy"}"#),
    }
}

/// A stopped server, as seen while tear down powers it off.
pub(super) fn stopped_server() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/listed-stopped-instance.json").to_owned(),
    }
}

/// A successful server deletion.
pub(super) fn deleted() -> Reply {
    Reply {
        status: 204,
        body: String::new(),
    }
}

/// An empty server listing, ending the disappearance poll.
pub(super) fn gone() -> Reply {
    Reply {
        status: 200,
        body: String::from(r#"{"servers":[]}"#),
    }
}

/// Returns the base request with the cache volume configured.
pub(super) fn request_with_volume(base: &InstanceRequest) -> InstanceRequest {
    let mut request = base.clone();
    request.volume_id = Some(String::from(CACHE_ID));
    request
}

/// Returns the captured request at `index`.
pub(super) fn at(requests: &[String], index: usize) -> &String {
    requests
        .get(index)
        .expect("expected a captured request at this index")
}

/// Builds a volume reply whose project differs from the configured one.
pub(super) fn with_project(project: &str) -> Reply {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/block-volume-available.json"))
            .expect("volume fixture is JSON");
    if let Some(object) = value.as_object_mut() {
        object.insert(
            String::from("project_id"),
            serde_json::Value::String(project.to_owned()),
        );
    }
    Reply {
        status: 200,
        body: value.to_string(),
    }
}

/// Scenario: creating a server with a cache volume.
///
/// Invariant: the lookup precedes the server POST, the attachment precedes the
/// power-on action, and the request order is exactly that sequence.
#[rstest]
#[tokio::test]
async fn create_with_image_id_attaches_before_power_on(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let request = request_with_volume(&base_request);
    let (result, requests) = with_stub(
        backend_fixture,
        vec![
            available_volume(),
            created_server(),
            attached_server(),
            in_use_volume(),
            action_ok(),
        ],
        async |backend| backend.create_with_image_id(&request, "img-test").await,
    )
    .await?;

    let handle = result?;
    assert_eq!(handle.id, "id");
    assert_eq!(handle.zone, "zone");
    assert_eq!(requests.len(), 5);

    assert_eq!(
        request_url(at(&requests, 0))?.path(),
        format!("/block/v1/zones/zone/volumes/{CACHE_ID}")
    );
    assert!(at(&requests, 1).starts_with("POST /instance/v1/zones/zone/servers "));
    assert_eq!(
        request_url(at(&requests, 2))?.path(),
        "/instance/v1/zones/zone/servers/id/attach-volume"
    );
    assert_eq!(
        request_body(at(&requests, 2))?,
        serde_json::json!({"volume_id": CACHE_ID, "volume_type": "sbs_volume"})
    );
    assert!(at(&requests, 3).starts_with("GET /block/v1/zones/zone/volumes/"));
    assert_eq!(
        request_url(at(&requests, 4))?.path(),
        "/instance/v1/zones/zone/servers/id/action"
    );
    assert_eq!(
        request_body(at(&requests, 4))?,
        serde_json::json!({"action": "poweron"})
    );
    Ok(())
}

/// Scenario: creating a server without a cache volume.
///
/// Invariant: no Block Storage request is made and the server is powered on
/// immediately after creation.
#[rstest]
#[tokio::test]
async fn create_with_image_id_without_a_volume_skips_the_block_api(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![created_server(), action_ok()],
        async |backend| {
            backend
                .create_with_image_id(&base_request, "img-test")
                .await
        },
    )
    .await?;

    result?;
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| !request.contains("/block/v1"))
    );
    Ok(())
}
