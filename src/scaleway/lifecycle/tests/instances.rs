//! Tests through the real instance creation wrapper and a loopback Scaleway
//! API.
//!
//! Creation must reach the substitutable API base rather than the SDK's
//! hard-coded production host, must name the server after the allocation tag,
//! and must send that tag as a provider tag so an interrupted allocation can be
//! rediscovered. A hard-coded base, a dropped `stopped` flag, or a missing
//! allocation tag therefore fails here instead of in a live account.

use anyhow::{Context, Result};
use rstest::rstest;

use super::{
    backend_fixture,
    base_request,
    http_stub::{Reply, only_request, request_body, request_url, with_stub},
    ledger,
};
use crate::backend::InstanceRequest;

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

/// Scenario: creating an instance before its cache volume is attached.
///
/// Invariant: the POST targets the configured zone on the substitutable base
/// and sends a stopped server tagged as ephemeral, carrying the allocation tag,
/// and named after that tag.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_posts_a_stopped_server(
    backend_fixture: crate::scaleway::ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let ledger = ledger();
    let tag = ledger.tag();
    let expected_name = format!("mriya-{}", tag.suffix());
    let (result, requests) =
        with_stub(backend_fixture, vec![created_instance()], async |backend| {
            backend
                .create_instance_stopped(&base_request, "img-test", &ledger)
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
    assert_eq!(body.get("name"), Some(&serde_json::json!(expected_name)));
    assert_eq!(
        without_name(&body)?,
        serde_json::json!({
            "commercial_type": "type",
            "image": "img-test",
            "project": "proj",
            "routed_ip_enabled": true,
            "dynamic_ip_required": true,
            "tags": ["mriya", "ephemeral", tag.as_str()],
            "stopped": true,
        })
    );
    Ok(())
}

/// Returns the creation payload minus its `name` field.
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
/// Invariant: a 404 survives as a `NotFound` error naming the operation rather
/// than being misreported as an unavailable instance type or a generic
/// provider failure.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_preserves_a_provider_rejection(
    backend_fixture: crate::scaleway::ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let ledger = ledger();
    let (result, requests) = with_stub(backend_fixture, vec![rejected()], async |backend| {
        backend
            .create_instance_stopped(&base_request, "img-test", &ledger)
            .await
    })
    .await?;
    let error = result.expect_err("a rejected creation must fail");
    assert!(
        matches!(
            error,
            crate::scaleway::ScalewayBackendError::NotFound {
                ref operation,
                ref message,
            } if operation == "create server" && message.contains("no such image")
        ),
        "{error:?}"
    );
    assert_eq!(requests.len(), 1);
    Ok(())
}
