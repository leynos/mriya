//! Tests through the real instance creation wrapper and a loopback Scaleway
//! API.
//!
//! Creation must reach the substitutable API base rather than the SDK's
//! hard-coded production host, and the server it posts must start stopped so
//! cloud-init user-data is in place before first boot. A hard-coded base or a
//! dropped `stopped` flag therefore fails here instead of in a live account.

use anyhow::{Context, Result};
use rstest::rstest;

use super::backend_fixture;
use super::base_request;
use super::http_stub::{Reply, StubApi};
use crate::backend::InstanceRequest;
use crate::scaleway::{ScalewayBackend, ScalewayBackendError};

/// Represents a successful creation, reusing the single-server fixture the
/// detach path also decodes.
fn created_instance() -> Reply {
    Reply {
        status: 201,
        body: include_str!("fixtures/detach-instance.json").to_owned(),
    }
}

/// Represents a structured provider rejection that does not identify the
/// instance type.
fn rejected() -> Reply {
    Reply {
        status: 404,
        body: String::from(r#"{"type":"denied","message":"no such image"}"#),
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

/// Scenario: creating an instance before its cache volume is attached.
///
/// Invariant: the POST targets the configured zone on the substitutable base
/// and sends a stopped server tagged as ephemeral, and the provider's server
/// is returned to the caller.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_posts_a_stopped_server(
    mut backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let stub = StubApi::start(vec![created_instance()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let server = backend_fixture
        .create_instance_stopped(&base_request, "img-test")
        .await?;
    assert_eq!(server.id, "id");
    let requests = stub.finish().await?;
    assert_eq!(requests.len(), 1);
    let request = requests.first().context("creation request missing")?;
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
    // whole. That also proves the optional fields stay absent.
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
        })
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
    mut backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let stub = StubApi::start(vec![rejected()]).await?;
    backend_fixture.instance_api_base.clone_from(&stub.base_url);
    let error = backend_fixture
        .create_instance_stopped(&base_request, "img-test")
        .await
        .expect_err("a rejected creation must fail");
    assert!(
        matches!(error, ScalewayBackendError::Provider { ref message } if message.contains("no such image")),
        "{error:?}"
    );
    assert_eq!(stub.finish().await?.len(), 1);
    Ok(())
}
