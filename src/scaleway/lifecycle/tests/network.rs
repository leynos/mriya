//! Tests through the real lifecycle wrappers and a loopback Scaleway API.
//!
//! Reads, readiness, absence checks, and actions all reach the substitutable
//! base. Absence is only ever confirmed by a successful, identity-checked
//! listing, so a 404 alone must not settle it.

use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result};
use rstest::rstest;
use tokio::net::TcpListener;

use super::{
    backend_fixture,
    base_request,
    fake_api::FakeApi,
    handle,
    http_stub::{Reply, StubApi, request_url},
    ledger,
};
use crate::{
    backend::{Backend, InstanceHandle},
    ownership::ResourceRef,
    scaleway::{
        ScalewayBackend,
        ScalewayBackendError,
        lifecycle::models::Presence,
        types::{Action, Zone},
    },
};

/// A successful provider response with no listed instances.
fn empty_list() -> Reply {
    Reply {
        status: 200,
        body: String::from("{\"servers\":[]}"),
    }
}

/// Loads the complete single-server response fixture the lifecycle reads.
fn single_instance() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/detach-instance.json").to_owned(),
    }
}

/// Loads the complete server-list response fixture.
fn listed_instance() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/listed-instance.json").to_owned(),
    }
}

/// Represents a structured provider denial for error-preservation checks.
fn provider_error() -> Reply {
    Reply {
        status: 403,
        body: String::from("{\"type\":\"denied\",\"message\":\"no access\"}"),
    }
}

/// Represents a 404 for a server that may or may not still exist.
fn not_found() -> Reply {
    Reply {
        status: 404,
        body: String::from(r#"{"type":"not_found","message":"no such server"}"#),
    }
}

/// Checks snapshot mapping through the direct server read.
#[rstest]
#[tokio::test]
async fn fetch_instance_maps_the_provider_snapshot(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let stub = StubApi::start(vec![single_instance()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let snapshot = backend_fixture
        .fetch_instance(&handle)
        .await?
        .context("instance missing")?;
    assert_eq!(snapshot.id.as_str(), "id");
    assert_eq!(snapshot.state.as_str(), "running");
    assert_eq!(snapshot.allowed_actions, vec![Action::from("reboot")]);
    assert_eq!(snapshot.public_ip.as_deref(), Some("127.0.0.1"));
    let requests = stub.finish().await?;
    let request = requests.first().context("read request missing")?;
    assert!(request.starts_with("GET "), "{request}");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-auth-token: dummy\r\n")
    );
    assert_eq!(
        request_url(request)?.path(),
        "/instance/v1/zones/zone/servers/id"
    );
    Ok(())
}

/// Exercises readiness through provider polling and a listening loopback SSH
/// port.
#[rstest]
#[tokio::test]
async fn wait_for_ready_fetches_and_checks_ssh(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let stub = StubApi::start(vec![single_instance()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    backend_fixture.ssh_port = address.port();
    backend_fixture.wait_timeout = Duration::from_secs(2);
    let networking = backend_fixture.wait_for_ready(&handle).await?;
    assert_eq!(networking.public_ip, address.ip());
    assert_eq!(networking.ssh_port, address.port());
    let accepted = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await??;
    drop(accepted);
    assert_eq!(stub.finish().await?.len(), 1);
    Ok(())
}

/// Scenario: a recorded server returns 404.
///
/// Invariant: absence is confirmed only by a successful listing in the recorded
/// zone and project, so the 404 alone does not settle the check.
#[rstest]
#[tokio::test]
async fn server_presence_confirms_absence_by_listing(
    mut backend_fixture: ScalewayBackend,
) -> Result<()> {
    let stub = StubApi::start(vec![not_found(), empty_list()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let resource = ResourceRef::server("id", "zone");
    // A project other than the fixture default proves the caller's project is
    // the one the confirmation listing uses.
    let presence = backend_fixture
        .server_presence(&resource, "other-project")
        .await?;
    assert!(
        matches!(presence, Presence::ConfirmedAbsent),
        "{presence:?}"
    );

    let requests = stub.finish().await?;
    assert_eq!(requests.len(), 2);
    let list = requests.get(1).context("list request missing")?;
    let url = request_url(list)?;
    assert_eq!(url.path(), "/instance/v1/zones/zone/servers");
    let query = url.query_pairs().collect::<BTreeMap<_, _>>();
    assert_eq!(
        query.get("project").map(AsRef::as_ref),
        Some("other-project")
    );
    assert_eq!(query.get("page").map(AsRef::as_ref), Some("1"));
    Ok(())
}

/// Scenario: a 404 after a listing that still contains the server.
///
/// Invariant: the server is reported present, because the listing is the
/// authority and the direct read may have raced a transient failure.
#[rstest]
#[tokio::test]
async fn server_presence_finds_a_server_the_read_missed(
    mut backend_fixture: ScalewayBackend,
) -> Result<()> {
    // Page 2 is empty, which is what ends pagination after the matching page.
    let stub = StubApi::start(vec![not_found(), listed_instance(), empty_list()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let resource = ResourceRef::server("id", "zone");
    let presence = backend_fixture.server_presence(&resource, "proj").await?;
    assert!(matches!(presence, Presence::Present(_)), "{presence:?}");
    Ok(())
}

/// Scenario: a listing returns a server with a different identifier.
///
/// Invariant: absence for the requested identifier is decided by comparing ids
/// locally, so a foreign or mangled identifier in the listing cannot make a
/// missing server look present.
#[rstest]
#[tokio::test]
async fn server_presence_rejects_a_listed_foreign_id() -> Result<()> {
    let api = FakeApi::start().await?;
    let mut backend = backend_fixture();
    backend.instance_api_base = String::from(api.instance_base());
    let owned = ledger();
    backend.create(&base_request(), &owned).await?;

    api.configure(|knobs| knobs.wrong_list_id = true);
    let missing = ResourceRef::server("srv-does-not-exist", "zone");
    let presence = backend.server_presence(&missing, "proj").await?;
    assert!(
        matches!(presence, Presence::ConfirmedAbsent),
        "{presence:?}"
    );
    Ok(())
}

/// Ensures structured provider errors remain visible to lifecycle callers.
#[rstest]
#[tokio::test]
async fn fetch_instance_preserves_provider_errors(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let stub = StubApi::start(vec![provider_error()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let result = backend_fixture.fetch_instance(&handle).await;
    assert!(
        matches!(
            result,
            Err(ScalewayBackendError::AccessDenied { status: 403, .. })
        ),
        "{result:?}"
    );
    assert_eq!(stub.finish().await?.len(), 1);
    Ok(())
}

/// Checks that the power-on wrapper sends the expected action request locally.
#[rstest]
#[tokio::test]
async fn power_on_uses_the_stubbed_action_endpoint(
    mut backend_fixture: ScalewayBackend,
) -> Result<()> {
    let stub = StubApi::start(vec![Reply {
        status: 202,
        body: String::from("{}"),
    }])
    .await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let snapshot = super::snapshot("id", "stopped", [Action::from("poweron")], None);
    backend_fixture
        .power_on_if_needed(&Zone::from("zone"), &snapshot)
        .await?;
    let requests = stub.finish().await?;
    assert_eq!(requests.len(), 1);
    let request = requests.first().context("action request missing")?;
    assert!(
        request.starts_with("POST /instance/v1/zones/zone/servers/id/action "),
        "{request}"
    );
    assert!(request.ends_with("{\"action\":\"poweron\"}"), "{request}");
    Ok(())
}
