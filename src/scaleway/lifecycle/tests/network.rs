//! Tests through the real lifecycle wrappers and a loopback Scaleway API.

use std::time::Duration;

use anyhow::{Context, Result};
use rstest::rstest;
use tokio::net::TcpListener;

use super::http_stub::{Reply, StubApi};
use super::{backend_fixture, handle};
use crate::backend::{Backend, InstanceHandle};
use crate::scaleway::types::{Action, Zone};
use crate::scaleway::{ScalewayBackend, ScalewayBackendError};

fn empty_list() -> Reply {
    Reply {
        status: 200,
        body: String::from("{\"servers\":[]}"),
    }
}

fn listed_instance() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/listed-instance.json").to_owned(),
    }
}

fn provider_error() -> Reply {
    Reply {
        status: 403,
        body: String::from("{\"type\":\"denied\",\"message\":\"no access\"}"),
    }
}

fn request_url(request: &str) -> Result<reqwest::Url> {
    let path = request
        .split_whitespace()
        .nth(1)
        .context("request path missing")?;
    reqwest::Url::parse(&format!("http://localhost{path}")).context("invalid request URL")
}

#[rstest]
#[tokio::test]
async fn fetch_instance_maps_the_provider_snapshot(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let stub = StubApi::start(vec![listed_instance(), empty_list()]).await?;
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
    assert_eq!(requests.len(), 2);
    for (index, request) in requests.iter().enumerate() {
        assert!(request.starts_with("GET "), "{request}");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-auth-token: dummy\r\n")
        );
        let url = request_url(request)?;
        assert_eq!(url.path(), "/instance/v1/zones/zone/servers");
        let query = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(query.get("servers").map(AsRef::as_ref), Some("id"));
        assert_eq!(query.get("per_page").map(AsRef::as_ref), Some("1"));
        assert_eq!(
            query.get("page").map(AsRef::as_ref),
            Some((index + 1).to_string().as_str())
        );
    }
    Ok(())
}

#[rstest]
#[tokio::test]
async fn wait_for_ready_fetches_and_checks_ssh(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let stub = StubApi::start(vec![listed_instance(), empty_list()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    backend_fixture.ssh_port = address.port();
    backend_fixture.wait_timeout = Duration::from_secs(2);
    let networking = backend_fixture.wait_for_ready(&handle).await?;
    assert_eq!(networking.public_ip, address.ip());
    assert_eq!(networking.ssh_port, address.port());
    let accepted = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await??;
    drop(accepted);
    assert_eq!(stub.finish().await?.len(), 2);
    Ok(())
}

#[rstest]
#[case::gone(false)]
#[case::residual(true)]
#[tokio::test]
async fn destroy_deletes_then_waits_for_disappearance(
    mut backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
    #[case] residual: bool,
) -> Result<()> {
    let mut replies = vec![Reply {
        status: 204,
        body: String::new(),
    }];
    if residual {
        replies.push(listed_instance());
    }
    replies.push(empty_list());
    let stub = StubApi::start(replies).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    // One initial fetch completes before the budget; the next tick is beyond
    // it, so a remaining resource must fail instead of skipping polling.
    backend_fixture.wait_timeout = Duration::from_secs(2);
    if residual {
        backend_fixture.poll_interval = Duration::from_secs(3);
    }
    let result = backend_fixture.destroy(handle).await;
    if residual {
        assert!(
            matches!(result, Err(ScalewayBackendError::ResidualResource { ref instance_id }) if instance_id == "id"),
            "{result:?}"
        );
    } else {
        result?;
    }
    let requests = stub.finish().await?;
    let first = requests.first().context("DELETE missing")?;
    assert!(
        first.starts_with("DELETE /instance/v1/zones/zone/servers/id "),
        "{first}"
    );
    assert!(
        requests
            .iter()
            .skip(1)
            .all(|request| request.starts_with("GET "))
    );
    assert_eq!(requests.len(), if residual { 3 } else { 2 });
    Ok(())
}

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
        matches!(result, Err(ScalewayBackendError::Provider { ref message }) if message == "Api: denied - no access"),
        "{result:?}"
    );
    assert_eq!(stub.finish().await?.len(), 1);
    Ok(())
}

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
