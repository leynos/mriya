//! Tests for the zone product-catalogue lookup.
//!
//! The catalogue describes every type in a zone, including shapes this backend
//! does not model. These cases pin three things a real account depends on: an
//! unrelated unfamiliar entry must not break the lookup, pagination must find
//! a type on a later page, and a page that makes no progress must end the loop
//! rather than spin forever.

use super::{
    backend_fixture,
    http_stub::{Reply, request_url, with_stub},
};
use crate::scaleway::ScalewayBackendError;

/// A catalogue entry shaped like the documented `ServerType`.
fn dev1_s_entry() -> serde_json::Value {
    serde_json::json!({
        "volumes_constraint": {"min_size": 10_000_000_000u64, "max_size": 20_000_000_000u64},
        "per_volume_constraint": {
            "l_ssd": {"min_size": 10_000_000_000u64, "max_size": 20_000_000_000u64}
        },
        "arch": "x86_64",
        "end_of_service": false,
        "capabilities": {"block_storage": true}
    })
}

/// One catalogue page carrying `servers`.
fn page(servers: &serde_json::Value) -> Reply {
    Reply {
        status: 200,
        body: serde_json::json!({ "servers": servers }).to_string(),
    }
}

/// Scenario: the requested type is the only well-formed entry.
///
/// Invariant: an unrelated entry of unfamiliar shape does not stop the lookup
/// decoding the type that was actually asked for, because entries are decoded
/// one at a time.
#[tokio::test]
async fn an_unfamiliar_sibling_entry_does_not_break_the_lookup() {
    let catalogue = page(&serde_json::json!({
        "SOMETHING-ELSE": {"shape": "not modelled here"},
        "DEV1-S": dev1_s_entry(),
    }));
    let (result, requests) = with_stub(backend_fixture(), vec![catalogue], async |backend| {
        backend.fetch_server_type("zone", "DEV1-S").await
    })
    .await
    .expect("the stub should serve the catalogue");

    let product = result.expect("the type should resolve");
    assert_eq!(product.arch, "x86_64");
    assert_eq!(product.volumes_constraint.max_size, 20_000_000_000);
    assert!(product.supports_block_storage());
    let request = super::http_stub::only_request(&requests).expect("one request was captured");
    assert_eq!(
        request_url(request)
            .expect("the request URL should parse")
            .path(),
        "/instance/v1/zones/zone/products/servers"
    );
}

/// Scenario: the requested type is absent from a catalogue that has entries.
///
/// Invariant: the error names the type and the zone rather than claiming a
/// decoding failure, so an operator can tell an unavailable type from a
/// changed payload.
#[tokio::test]
async fn an_absent_type_reports_itself() {
    // The first page lists another type, so pagination asks for a second; the
    // empty second page ends it without the requested type appearing.
    let catalogue = page(&serde_json::json!({"GP1-XS": dev1_s_entry()}));
    let (result, _) = with_stub(
        backend_fixture(),
        vec![catalogue, page(&serde_json::json!({}))],
        async |backend| backend.fetch_server_type("zone", "DEV1-S").await,
    )
    .await
    .expect("the stub should serve the catalogue");

    let error = result.expect_err("an absent type must be reported");
    assert!(
        matches!(
            error,
            ScalewayBackendError::ProductCatalogueUnavailable {
                ref instance_type,
                ref zone,
            } if instance_type == "DEV1-S" && zone == "zone"
        ),
        "{error:?}"
    );
}

/// Scenario: the type appears on the second page.
///
/// Invariant: pagination follows the pages until the type is found, so a
/// zone's catalogue size does not decide whether a type resolves.
#[tokio::test]
async fn a_type_on_a_later_page_is_found() {
    let first = page(&serde_json::json!({"GP1-XS": dev1_s_entry()}));
    let second = page(&serde_json::json!({"DEV1-S": dev1_s_entry()}));
    let (result, requests) = with_stub(backend_fixture(), vec![first, second], async |backend| {
        backend.fetch_server_type("zone", "DEV1-S").await
    })
    .await
    .expect("the stub should serve the catalogue");

    result.expect("the type should resolve");
    assert_eq!(requests.len(), 2, "the lookup should have read two pages");
    let pages: Vec<String> = requests
        .iter()
        .map(|request| {
            request_url(request)
                .expect("the request URL should parse")
                .query_pairs()
                .find(|(key, _)| key == "page")
                .map(|(_, value)| value.into_owned())
                .expect("the request should carry a page")
        })
        .collect();
    assert_eq!(
        pages,
        vec![String::from("1"), String::from("2")],
        "the lookup must request successive pages"
    );
}

/// Scenario: the endpoint keeps returning the same page.
///
/// Invariant: a page that adds no new key ends the loop, so an endpoint that
/// ignores the `page` parameter produces an error instead of spinning.
#[tokio::test]
async fn a_repeating_page_ends_the_lookup() {
    let repeated = || page(&serde_json::json!({"GP1-XS": dev1_s_entry()}));
    let (result, requests) = with_stub(
        backend_fixture(),
        vec![repeated(), repeated()],
        async |backend| backend.fetch_server_type("zone", "DEV1-S").await,
    )
    .await
    .expect("the stub should serve the catalogue");

    let error = result.expect_err("a repeating page must end the lookup");
    assert!(
        matches!(
            error,
            ScalewayBackendError::ProductCatalogueUnavailable { .. }
        ),
        "{error:?}"
    );
    assert_eq!(
        requests.len(),
        2,
        "the second page adds nothing, so the loop must stop there"
    );
}

/// Scenario: the requested type's entry is unreadable.
///
/// Invariant: the failure names the decoding problem, so a changed payload is
/// distinguishable from an absent type.
#[tokio::test]
async fn an_unreadable_entry_reports_the_decoding_failure() {
    let catalogue = page(&serde_json::json!({"DEV1-S": {"unexpected": true}}));
    let (result, _) = with_stub(backend_fixture(), vec![catalogue], async |backend| {
        backend.fetch_server_type("zone", "DEV1-S").await
    })
    .await
    .expect("the stub should serve the catalogue");

    let error = result.expect_err("an unreadable entry must fail the lookup");
    let message = error.to_string();
    assert!(message.contains("DEV1-S"), "{message}");
    assert!(message.contains("unreadable catalogue entry"), "{message}");
}
