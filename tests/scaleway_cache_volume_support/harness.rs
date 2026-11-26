//! Opt-in gates, configuration and shared runtime for the live cache suite.
//!
//! A disabled suite returns `Ok(None)`. Once the suite is enabled and both
//! credentials are present, a configuration or backend error is returned as a
//! setup failure so an invalid live configuration cannot silently skip the
//! persistence check.

use std::sync::{Arc, LazyLock, Mutex};

use anyhow::Result;
use mockable::{DefaultEnv, Env};
use mriya::{ScalewayBackend, ScalewayConfig, SyncConfig, janitor::TEST_RUN_ID_ENV};
use rstest::fixture;
use tokio::runtime::Runtime;

static RUNTIME: LazyLock<Result<Runtime, std::io::Error>> = LazyLock::new(Runtime::new);

/// Reports whether the live suite was opted into with a test-run id.
fn integration_enabled() -> bool {
    let enabled = DefaultEnv
        .raw("MRIYA_RUN_SCALEWAY_TESTS")
        .unwrap_or_default();
    let opted_in = matches!(
        enabled.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    );
    let has_run_id = DefaultEnv
        .raw(TEST_RUN_ID_ENV)
        .is_ok_and(|value| !value.trim().is_empty());
    opted_in && has_run_id
}

/// Live configuration and the SSH settings used to drive the guest.
#[derive(Clone)]
pub(super) struct CacheHarness {
    pub(super) backend: ScalewayBackend,
    pub(super) config: ScalewayConfig,
    pub(super) sync_config: SyncConfig,
}

/// Loads the live harness, distinguishing a disabled suite from a broken one.
///
/// # Errors
///
/// Returns a setup message when the suite is enabled with credentials but the
/// configuration, backend or sync settings cannot be loaded.
#[fixture]
pub(super) fn cache_harness() -> Result<Option<CacheHarness>, String> {
    if !integration_enabled() {
        return Ok(None);
    }
    let secret = DefaultEnv
        .raw("SCW_SECRET_KEY")
        .map_err(|error| format!("SCW_SECRET_KEY: {error}"))?;
    let project = DefaultEnv
        .raw("SCW_DEFAULT_PROJECT_ID")
        .map_err(|error| format!("SCW_DEFAULT_PROJECT_ID: {error}"))?;
    if secret.trim().is_empty() || project.trim().is_empty() {
        return Ok(None);
    }

    let config = ScalewayConfig::load_from_sources()
        .map_err(|error| format!("live Scaleway configuration: {error}"))?;
    let backend = ScalewayBackend::new(config.clone())
        .map_err(|error| format!("live Scaleway backend: {error}"))?;
    let sync_config = SyncConfig::load_without_cli_args()
        .map_err(|error| format!("live sync configuration: {error}"))?;
    Ok(Some(CacheHarness {
        backend,
        config,
        sync_config,
    }))
}

/// Runs one async block on the shared Tokio runtime.
///
/// # Errors
///
/// Returns the future's error, or a runtime-construction failure.
pub(super) fn block_on<T, F>(future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    match RUNTIME.as_ref() {
        Ok(runtime) => runtime.block_on(future),
        Err(error) => Err(anyhow::anyhow!("tokio runtime should start: {error}")),
    }
}

/// Volume size used by the live check: the provider minimum.
pub(super) const VOLUME_SIZE_BYTES: u64 = 5_000_000_000;
/// How long to wait for a deleted volume to disappear.
pub(super) const DELETE_WAIT: std::time::Duration = std::time::Duration::from_secs(120);
/// Delay between deletion checks.
pub(super) const DELETE_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Evidence gathered by the three-VM flow.
#[derive(Clone, Debug)]
pub(super) struct CacheEvidence {
    pub(super) sentinel_a: String,
    pub(super) sentinel_b: String,
    pub(super) read_a_on_second: bool,
    pub(super) read_b_on_third: bool,
    /// Volume status observed after each VM teardown, in order.
    pub(super) volume_statuses: Vec<String>,
    pub(super) volume_gone: bool,
}

/// Evidence slot written by the `when` step and read by the `then` steps.
///
/// `rstest-bdd` stores a step's return value only when exactly one scenario
/// fixture has the same type, so the flow records its evidence in this
/// explicitly typed slot instead of relying on an implicit type match.
#[derive(Clone)]
pub(super) struct CacheEvidenceSlot {
    /// Evidence captured by the persistence flow, once it has run.
    evidence: Arc<Mutex<Option<CacheEvidence>>>,
}

impl CacheEvidenceSlot {
    /// Builds an empty slot.
    pub(super) fn new() -> Self {
        Self {
            evidence: Arc::new(Mutex::new(None)),
        }
    }

    /// Stores the evidence produced by the flow.
    pub(super) fn set(&self, evidence: CacheEvidence) {
        if let Ok(mut slot) = self.evidence.lock() {
            *slot = Some(evidence);
        }
    }

    /// Returns a copy of the recorded evidence, if the flow has run.
    pub(super) fn get(&self) -> Option<CacheEvidence> {
        self.evidence.lock().ok().and_then(|slot| slot.clone())
    }
}

/// Scenario fixture holding the live flow's evidence.
#[mriya_test_macros::allow_fixture_expansion_lints]
#[fixture]
pub fn cache_evidence_slot() -> CacheEvidenceSlot { CacheEvidenceSlot::new() }

/// Failure surfaced by a live step.
#[derive(Debug, thiserror::Error)]
pub(super) enum StepError {
    /// Human-readable failure description.
    #[error("{0}")]
    Message(String),
}
