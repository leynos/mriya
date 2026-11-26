//! Tests through the real instance creation wrapper and a loopback Scaleway
//! API.
//!
//! Creation must reach the substitutable API base rather than the SDK's
//! hard-coded production host, and the server it posts must start stopped so
//! cloud-init user-data is in place before first boot. A hard-coded base or a
//! dropped `stopped` flag therefore fails here instead of in a live account.

use anyhow::{Context, Result};
use rstest::rstest;

use super::{
    backend_fixture,
    base_request,
    http_stub::{Reply, only_request, request_body, request_url, with_stub},
};
use crate::{
    backend::InstanceRequest,
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

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
/// and sends a stopped server tagged as ephemeral, and the provider's server
/// is returned to the caller.
#[rstest]
#[tokio::test]
async fn create_instance_stopped_posts_a_stopped_server(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let (result, requests) =
        with_stub(backend_fixture, vec![created_instance()], async |backend| {
            backend
                .create_instance_stopped(&base_request, "img-test")
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
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![rejected()], async |backend| {
        backend
            .create_instance_stopped(&base_request, "img-test")
            .await
    })
    .await?;
    let error = result.expect_err("a rejected creation must fail");
    assert!(
        matches!(error, ScalewayBackendError::Provider { ref message } if message.contains("no such image")),
        "{error:?}"
    );
    assert_eq!(requests.len(), 1);
    Ok(())
}

/// A create response whose body is not a server, so the call reports a
/// provider error after the provider has already allocated the resource.
fn created_but_unreadable() -> Reply {
    Reply {
        status: 201,
        body: String::from(r#"{"servers":"not what the schema says"}"#),
    }
}

/// Scenario: a successful creation reports the resource it created.
///
/// Invariant: the lifecycle sink receives one `ResourceCreated` event whose
/// identifier and location are the ones the provider returned, so a benchmark
/// record can name the resource instead of guessing an identifier.
#[rstest]
#[tokio::test]
async fn a_successful_creation_reports_the_resource_it_created(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    use crate::bench::lifecycle::{LifecycleEvent, RecordingSink};

    let sink = std::sync::Arc::new(RecordingSink::default());
    let backend = backend_fixture.with_lifecycle_sink(sink.clone());
    let (result, _) = with_stub(backend, vec![created_instance()], async |stubbed| {
        stubbed
            .create_instance_stopped_observed(&base_request, "img-test")
            .await
    })
    .await?;
    result?;

    assert_eq!(
        sink.events(),
        vec![LifecycleEvent::ResourceCreated {
            provider: String::from("scaleway"),
            resource_id: String::from("id"),
            location: String::from("zone"),
            product: String::from("type"),
        }]
    );
    Ok(())
}

/// Scenario: a create call whose response cannot be read.
///
/// Invariant: the sink receives an `AllocationUncertain` event rather than a
/// `ResourceCreated` event, because the provider may have allocated a resource
/// whose identifier the client never learned. That is the difference between a
/// campaign that can sweep its leftovers and one that cannot.
#[rstest]
#[tokio::test]
async fn an_unreadable_create_response_reports_an_uncertain_allocation(
    backend_fixture: ScalewayBackend,
    base_request: InstanceRequest,
) -> Result<()> {
    use crate::bench::lifecycle::{LifecycleEvent, RecordingSink};

    let sink = std::sync::Arc::new(RecordingSink::default());
    let backend = backend_fixture.with_lifecycle_sink(sink.clone());
    let (result, _) = with_stub(backend, vec![created_but_unreadable()], async |stubbed| {
        stubbed
            .create_instance_stopped_observed(&base_request, "img-test")
            .await
    })
    .await?;
    let error = result.expect_err("an unreadable body is a provider error");
    assert!(
        matches!(error, ScalewayBackendError::Provider { .. }),
        "{error:?}"
    );

    let events = sink.events();
    assert!(
        matches!(
            events.as_slice(),
            [LifecycleEvent::AllocationUncertain { provider, location, .. }]
                if provider == "scaleway" && location == "zone"
        ),
        "expected one uncertain allocation, got {events:?}"
    );
    Ok(())
}

/// Scenario: a teardown that the provider confirms.
///
/// Invariant: the sink sees the delete request and then a verified teardown, in
/// that order, so a campaign can distinguish "we asked" from "the provider
/// confirmed nothing remains".
#[rstest]
#[tokio::test]
async fn a_verified_teardown_reports_both_events(backend_fixture: ScalewayBackend) -> Result<()> {
    use crate::{
        Backend,
        backend::InstanceHandle,
        bench::lifecycle::{LifecycleEvent, RecordingSink},
    };

    let sink = std::sync::Arc::new(RecordingSink::default());
    let mut backend = backend_fixture.with_lifecycle_sink(sink.clone());
    // The delete response, then two empty listings that end pagination.
    let delete = Reply {
        status: 204,
        body: String::new(),
    };
    let empty_page = Reply {
        status: 200,
        body: String::from(r#"{"servers":[]}"#),
    };
    let stub = super::http_stub::StubApi::start(vec![delete, empty_page]).await?;
    backend.instance_api_base.clone_from(&stub.base_url);

    backend
        .destroy(InstanceHandle {
            id: String::from("srv-1"),
            zone: String::from("zone"),
        })
        .await?;
    stub.finish().await?;

    assert_eq!(
        sink.events(),
        vec![
            LifecycleEvent::TeardownRequested {
                provider: String::from("scaleway"),
                resource_id: String::from("srv-1"),
            },
            LifecycleEvent::TeardownVerified {
                provider: String::from("scaleway"),
                resource_id: String::from("srv-1"),
            },
        ]
    );
    Ok(())
}
