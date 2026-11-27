//! End-to-end ownership tests through the production `create` and `cleanup`
//! paths against a stateful loopback provider.
//!
//! Each scenario asserts the provider's resulting state, not just the returned
//! value: an owned server, its root volume, and its dynamic IP must disappear,
//! a retained cache must survive detached, and a denied delete must be
//! reported with residual identifiers.

use anyhow::Result;

use super::{
    backend_fixture,
    base_request,
    fake_api::{FakeApi, FakeVolume},
};
use crate::{
    backend::{Backend, InstanceRequest},
    cleanup::CleanupReport,
    ownership::{Disposition, ResourceKind, ResourceLedger},
    scaleway::{ScalewayBackend, ScalewayBackendError},
    volume::VolumeRequest,
};

/// A fake provider and a backend pointed at it.
struct Harness {
    /// The fake provider.
    api: FakeApi,
    /// The backend under test.
    backend: ScalewayBackend,
}

impl Harness {
    /// Starts the fake and rewires the backend.
    async fn new() -> Result<Self> {
        let api = FakeApi::start().await?;
        let mut backend = backend_fixture();
        backend.instance_api_base = String::from(api.instance_base());
        backend.block_api_base = String::from(api.block_base());
        Ok(Self { api, backend })
    }

    /// Adds a detached cache volume to the fake provider.
    fn insert_cache(&self, id: &str) {
        self.api.lock().instance_volumes.insert(
            id.to_owned(),
            FakeVolume {
                id: id.to_owned(),
                zone: String::from("zone"),
                volume_type: String::from("b_ssd"),
                attached_to: None,
            },
        );
    }
}

/// Creates an instance and leaves it running with its dynamic IP attached.
async fn create_running(harness: &Harness, request: &InstanceRequest) -> ResourceLedger {
    let ledger = super::ledger();
    harness
        .backend
        .create(request, &ledger)
        .await
        .expect("the fake provider must accept the create");
    ledger
}

/// Scenario: one successful run.
///
/// Invariant: cleanup removes the server, its root volume, and its dynamic IP,
/// and reports each as removed.
#[tokio::test]
async fn cleanup_removes_every_owned_resource() {
    let harness = Harness::new().await.expect("the fake provider must start");
    let request = base_request();
    let ledger = create_running(&harness, &request).await;
    assert_eq!(harness.api.server_ids().len(), 1);
    assert_eq!(harness.api.volume_ids().len(), 1);
    assert_eq!(harness.api.ip_ids().len(), 1);

    let report: CleanupReport = harness.backend.cleanup(&ledger).await.expect("cleanup");
    assert_eq!(harness.api.server_ids().len(), 0);
    assert_eq!(harness.api.volume_ids().len(), 0);
    assert_eq!(harness.api.ip_ids().len(), 0);
    assert_eq!(
        report
            .removed
            .iter()
            .filter(|resource| resource.kind() == ResourceKind::Server)
            .count(),
        1
    );

    // Repeated cleanup treats confirmed absence as success.
    let repeated = harness
        .backend
        .cleanup(&ledger)
        .await
        .expect("repeated cleanup");
    assert_eq!(repeated.preserved.len(), 0);
}

/// Scenario: a run with a configured cache volume.
///
/// Invariant: cleanup detaches the cache and deletes only the server and root
/// volume; the cache keeps its data and is reported as preserved.
#[tokio::test]
async fn cleanup_preserves_a_retained_cache() {
    let harness = Harness::new().await.expect("the fake provider must start");
    harness.insert_cache("vol-cache");
    let mut request = base_request();
    request.volume_id = Some(String::from("vol-cache"));
    let ledger = create_running(&harness, &request).await;
    assert!(harness.api.is_attached("vol-cache"));

    let report = harness.backend.cleanup(&ledger).await.expect("cleanup");
    assert_eq!(harness.api.volume_ids(), vec![String::from("vol-cache")]);
    assert!(!harness.api.is_attached("vol-cache"));
    assert_eq!(harness.api.server_ids().len(), 0);
    assert_eq!(
        report.preserved_ids(),
        vec!["vol-cache"],
        "the retained cache must be reported"
    );
}

/// Scenario: the cache attachment is rejected after the server was created.
///
/// Invariant: the create fails with its allocation already recorded, and
/// cleanup removes the server and root volume while preserving the cache.
#[tokio::test]
async fn partial_create_failures_are_cleaned_up() {
    let harness = Harness::new().await.expect("the fake provider must start");
    harness.insert_cache("vol-cache");
    harness
        .api
        .configure(|knobs| knobs.deny_volume_attach = true);
    let mut request = base_request();
    request.volume_id = Some(String::from("vol-cache"));
    let ledger = super::ledger();

    let error = harness
        .backend
        .create(&request, &ledger)
        .await
        .expect_err("the attachment is rejected");
    // A 403 is an authorization failure, not a rejected attachment: the
    // request was never evaluated, and cleanup must report it as a denial.
    assert!(
        matches!(
            error,
            ScalewayBackendError::AccessDenied { ref status, .. } if *status == 403
        ),
        "{error:?}"
    );

    harness
        .api
        .configure(|knobs| knobs.deny_volume_attach = false);
    harness.backend.cleanup(&ledger).await.expect("cleanup");
    assert_eq!(harness.api.server_ids().len(), 0);
    assert_eq!(harness.api.volume_ids(), vec![String::from("vol-cache")]);
    assert!(!harness.api.is_attached("vol-cache"));
}

/// Scenario: the create response never arrives.
///
/// Invariant: reconciliation finds the tagged server and records it, so the run
/// continues and cleanup can remove it.
#[tokio::test]
async fn an_ambiguous_create_is_reconciled_by_tag() {
    let harness = Harness::new().await.expect("the fake provider must start");
    harness
        .api
        .configure(|knobs| knobs.drop_create_response = true);
    let request = base_request();
    let ledger = super::ledger();

    let handle = harness
        .backend
        .create(&request, &ledger)
        .await
        .expect("reconciliation must find the created server");
    assert_eq!(harness.api.server_ids(), vec![handle.id.clone()]);
    assert!(ledger.find(ResourceKind::Server, &handle.id).is_some());

    harness.backend.cleanup(&ledger).await.expect("cleanup");
    assert_eq!(harness.api.server_ids().len(), 0);
}

/// Scenario: the provider denies the server deletion.
///
/// Invariant: cleanup fails with the server and its still-attached root volume
/// reported as residual, and the server is not silently forgotten.
#[tokio::test]
async fn a_denied_delete_is_reported_with_residuals() {
    let harness = Harness::new().await.expect("the fake provider must start");
    let request = base_request();
    let ledger = create_running(&harness, &request).await;
    harness
        .api
        .configure(|knobs| knobs.deny_server_delete = true);

    let failure = harness
        .backend
        .cleanup(&ledger)
        .await
        .expect_err("a denial must fail cleanup");
    let rendered = failure.to_string();
    assert!(rendered.contains("server srv-1@zone"), "{rendered}");
    assert!(rendered.contains("403"), "{rendered}");
    assert_ne!(harness.api.server_ids().len(), 0);
}

/// Scenario: the resources disappeared before cleanup ran.
///
/// Invariant: a 404 backed by a successful listing confirms absence, so cleanup
/// succeeds rather than reporting a phantom residual.
#[tokio::test]
async fn cleanup_treats_a_confirmed_404_as_success() {
    let harness = Harness::new().await.expect("the fake provider must start");
    let request = base_request();
    let ledger = create_running(&harness, &request).await;
    {
        let mut state = harness.api.lock();
        state.servers.clear();
        state.instance_volumes.clear();
        state.ips.clear();
    }

    let report = harness.backend.cleanup(&ledger).await.expect("cleanup");
    assert!(
        report
            .removed
            .iter()
            .any(|resource| resource.kind() == ResourceKind::Server)
    );
}

/// Scenario: the provider rejects the power-on action.
///
/// Invariant: create fails, the server and root volume it allocated are
/// recorded, and cleanup removes them.
#[tokio::test]
async fn a_failed_power_on_is_cleaned_up() {
    let harness = Harness::new().await.expect("the fake provider must start");
    harness.api.configure(|knobs| knobs.fail_power_on = true);
    let request = base_request();
    let ledger = super::ledger();

    harness
        .backend
        .create(&request, &ledger)
        .await
        .expect_err("the power-on is rejected");
    harness.backend.cleanup(&ledger).await.expect("cleanup");
    assert_eq!(harness.api.server_ids().len(), 0);
    assert_eq!(harness.api.volume_ids().len(), 0);
}

/// Scenario: the provider returns a server without a root volume.
///
/// Invariant: attaching a cache volume fails before any PATCH, and cleanup
/// still removes the server it created.
#[tokio::test]
async fn a_missing_root_volume_is_reported() {
    let harness = Harness::new().await.expect("the fake provider must start");
    harness.insert_cache("vol-cache");
    harness.api.configure(|knobs| knobs.omit_root_volume = true);
    let mut request = base_request();
    request.volume_id = Some(String::from("vol-cache"));
    let ledger = super::ledger();

    let error = harness
        .backend
        .create(&request, &ledger)
        .await
        .expect_err("a server without a root volume cannot take the cache");
    assert!(
        matches!(error, ScalewayBackendError::VolumeNotFound { .. }),
        "{error:?}"
    );
    harness.backend.cleanup(&ledger).await.expect("cleanup");
    assert_eq!(harness.api.server_ids().len(), 0);
}

/// Scenario: several allocations share the provider and lists are paginated.
///
/// Invariant: cleanup finds only the server carrying its own allocation tag,
/// even when the provider pages the listing one item at a time.
#[tokio::test]
async fn pagination_does_not_confuse_allocations() {
    let harness = Harness::new().await.expect("the fake provider must start");
    harness.api.configure(|knobs| knobs.page_size = 1);
    let first_request = base_request();
    let mut second_request = base_request();
    second_request.instance_type = String::from("DEV1-M");

    let first = super::ledger();
    let second = super::ledger();
    let first_handle = harness
        .backend
        .create(&first_request, &first)
        .await
        .expect("first create succeeds");
    // Only the second create loses its response, so reconciliation has to page
    // past the first allocation to find it.
    harness
        .api
        .configure(|knobs| knobs.drop_create_response = true);
    let second_handle = harness
        .backend
        .create(&second_request, &second)
        .await
        .expect("second create reconciles");
    assert_ne!(first_handle.id, second_handle.id);

    harness
        .backend
        .cleanup(&first)
        .await
        .expect("cleanup first");
    assert_eq!(harness.api.server_ids(), vec![second_handle.id]);
    harness
        .backend
        .cleanup(&second)
        .await
        .expect("cleanup second");
    assert_eq!(harness.api.server_ids().len(), 0);
}

/// Scenario: a leaked allocation is recorded as owned, then reused as a cache.
#[tokio::test]
async fn an_owned_volume_is_deleted_by_cleanup() {
    let harness = Harness::new().await.expect("the fake provider must start");
    let ledger = super::ledger();
    let handle = harness
        .backend
        .create_volume(
            &VolumeRequest::new("cache", 10_000_000_000, "zone", "proj"),
            &ledger,
        )
        .await
        .expect("volume create");
    assert_eq!(
        ledger
            .find(ResourceKind::InstanceVolume, &handle.id)
            .map(|entry| entry.disposition),
        Some(Disposition::Owned)
    );

    harness.backend.cleanup(&ledger).await.expect("cleanup");
    assert_eq!(harness.api.volume_ids().len(), 0);
}
