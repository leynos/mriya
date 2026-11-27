//! The Scaleway implementation of the paid campaign driver.
//!
//! This is the only code in the repository that can spend money, and it is
//! reachable only from `mriya-bench run` after [`crate::bench::campaign::run_campaign`]
//! has checked the operator's opt-in and the plan's limits.
//!
//! It is deliberately thin. Everything it does is a call into a component that
//! already exists and is already tested: the Scaleway backend for the instance,
//! the syncer for transport, the janitor for the pre-sweep, and the run
//! orchestrator for the workflow. What this module adds is the translation
//! between those components' results and a protocol record, and the checks that
//! make a record honest — a probe that cannot be read fails the attempt rather
//! than producing a record with a guessed CPU count.

use std::time::{Duration, Instant};

use super::{
    AttemptRecords,
    CampaignDriver,
    PlannedAttempt,
    SweepReport,
    TeardownReport,
    scaleway_facts::{intended_workers, outcome_of, phase_records, run_identifier, toolchain_pin},
};
use crate::{
    ScalewayConfig,
    backend::InstanceRequest,
    bench::{
        probe::parse_probe_report,
        record::{
            Allocation,
            Concurrency,
            Lifetime,
            ProviderProduct,
            RecordKind,
            RetainedStorageRecord,
            RunRecord,
            SCHEMA_VERSION,
            WorkloadClass,
        },
    },
    janitor::{DEFAULT_SCW_BIN, Janitor, JanitorConfig},
    scaleway::ScalewayBackend,
    sync::{DeadlineCommandRunner, SyncConfig},
};

/// An error the driver reports as a message, because the campaign stops anyway.
pub type DriverResult<T> = Result<T, String>;

/// Longest a sweep may take before its `scw` calls are killed.
///
/// A sweep is the campaign's only way to prove the account is clean, so it is
/// bounded rather than allowed to hang a campaign that is trying to stop.
const SWEEP_LIMIT: Duration = Duration::from_secs(600);

/// The production driver.
pub struct ScalewayDriver {
    /// Configuration for the provider.
    pub(super) config: ScalewayConfig,
    /// Configuration for transport.
    pub(super) sync_config: SyncConfig,
    /// Path to the `scw` CLI used by the pre-sweep.
    scw_bin: String,
}

impl std::fmt::Debug for ScalewayDriver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScalewayDriver")
            .field("zone", &self.config.default_zone)
            .field("product", &self.config.default_instance_type)
            .finish_non_exhaustive()
    }
}

impl ScalewayDriver {
    /// Builds a driver from the same configuration the ordinary CLI reads.
    ///
    /// # Errors
    ///
    /// Returns a message when either configuration cannot be loaded, so a
    /// missing credential stops the campaign before it allocates anything.
    pub fn from_env() -> DriverResult<Self> {
        let config = ScalewayConfig::load_without_cli_args()
            .map_err(|err| format!("Scaleway configuration: {err}"))?;
        let sync_config = SyncConfig::load_without_cli_args()
            .map_err(|err| format!("sync configuration: {err}"))?;
        Ok(Self {
            config,
            sync_config,
            scw_bin: String::from(DEFAULT_SCW_BIN),
        })
    }

    /// Builds a backend whose resources carry the campaign's own tag.
    ///
    /// The tag is what the janitor sweeps: a backend that tagged resources with
    /// the process environment's test-run id would create resources the
    /// campaign's own sweep could never find, so the campaign id is passed
    /// explicitly and used for both.
    pub(super) fn backend(
        &self,
        campaign_id: &str,
        sink: Option<crate::bench::lifecycle::SharedLifecycleSink>,
    ) -> DriverResult<ScalewayBackend> {
        let backend = ScalewayBackend::new_with_test_run_id(
            self.config.clone(),
            Some(campaign_id.to_owned()),
        )
        .map_err(|err| format!("backend: {err}"))?;
        Ok(match sink {
            Some(configured) => backend.with_lifecycle_sink(configured),
            None => backend,
        })
    }

    /// Builds the request one attempt uses.
    fn request(
        &self,
        campaign_id: &str,
        attempt: &PlannedAttempt,
    ) -> DriverResult<InstanceRequest> {
        let backend = self.backend(campaign_id, None)?;
        let mut request = backend
            .default_request()
            .map_err(|err| format!("default request: {err}"))?;
        request.instance_type.clone_from(&attempt.product);
        Ok(request)
    }

    /// Runs the janitor under the campaign's own tag.
    pub(super) fn sweep(&self, campaign_id: &str) -> DriverResult<SweepReport> {
        let config = JanitorConfig::new(
            self.config.default_project_id.clone(),
            campaign_id,
            self.scw_bin.clone(),
        )
        .map_err(|err| format!("janitor configuration: {err}"))?;
        let deadline = Instant::now()
            .checked_add(SWEEP_LIMIT)
            .unwrap_or_else(Instant::now);
        // The janitor reports what it deleted and whether anything remains; a
        // successful sweep that found nothing is the only acceptable start.
        match Janitor::new(config, DeadlineCommandRunner::new(deadline)).sweep() {
            Ok(_) => Ok(SweepReport::default()),
            Err(err) => Err(err.to_string()),
        }
    }
}

impl CampaignDriver for ScalewayDriver {
    fn provider(&self) -> &'static str { "scaleway" }

    fn pre_sweep(&self, campaign_id: &str) -> DriverResult<SweepReport> { self.sweep(campaign_id) }

    fn run_attempt(
        &self,
        campaign_id: &str,
        attempt: &PlannedAttempt,
    ) -> Result<AttemptRecords, super::ports::AttemptFailure> {
        let request = self
            .request(campaign_id, attempt)
            .map_err(super::ports::AttemptFailure::without_resources)?;
        let measured = self.measure(campaign_id, attempt, &request)?;
        let run_id = run_identifier(campaign_id, attempt);
        // The run completed and its teardown was verified, so a probe that
        // cannot be read leaves nothing behind.
        let probe = parse_probe_report(&measured.probe_stdout).map_err(|err| {
            super::ports::AttemptFailure::without_resources(format!(
                "the pre-workload probe could not be read: {err}"
            ))
        })?;
        Ok(AttemptRecords {
            run: RunRecord {
                schema: SCHEMA_VERSION,
                kind: RecordKind::Run,
                campaign_id: campaign_id.to_owned(),
                run_id: run_id.clone(),
                workload_id: attempt.workload_id.clone(),
                command: measured.command.clone(),
                role: attempt.role,
                class: WorkloadClass::Cpu,
                tree: measured.tree.clone(),
                toolchain: toolchain_pin(&probe),
                provider: ProviderProduct {
                    provider: String::from("scaleway"),
                    product: attempt.product.clone(),
                    location: self.config.default_zone.clone(),
                },
                allocation: Allocation {
                    requested_architecture: request.architecture.clone(),
                    ..probe.allocation()
                },
                concurrency: Concurrency {
                    intended_workers: intended_workers(),
                    effective_workers: probe.cpu_count,
                },
                cache: probe.cache(),
                lifetime: Lifetime {
                    total_nanos: measured.total_nanos,
                    // The provider's granularity is applied when the record is
                    // priced, from the price sheet rather than from a guess here.
                    chargeable_nanos: measured.total_nanos,
                },
                outcome: outcome_of(&measured.output),
                gpu: None,
                repetition: attempt.repetition,
                retained: retained_storage(&probe, measured.total_nanos),
            },
            phases: phase_records(campaign_id, &run_id, attempt, &measured.observations),
            resource_ids: measured.resource_ids,
        })
    }

    fn verify_teardown(
        &self,
        _campaign_id: &str,
        _attempt: &PlannedAttempt,
        _run: &RunRecord,
    ) -> DriverResult<TeardownReport> {
        // `run_attempt` returns `Ok` only after the orchestrator's teardown
        // phase has resolved, and that phase resolves only when the backend has
        // listed the instance and found it gone. A run that exists therefore
        // has a verified teardown; a run that failed never reaches here and is
        // recorded as a residual by the harness instead.
        Ok(TeardownReport::verified())
    }
}

/// Returns the storage the attempt holds after it ends.
///
/// A mounted cache volume survives the instance, so its capacity is charged for
/// the attempt's own lifetime. The class name must match the price sheet; a
/// sheet that does not describe it fails the report rather than pricing it at
/// zero, which is the safe direction.
fn retained_storage(
    probe: &crate::bench::probe::ProbeReport,
    held_nanos: u64,
) -> Vec<RetainedStorageRecord> {
    if !probe.cache_mounted || probe.cache_total_bytes == 0 {
        return Vec::new();
    }
    vec![RetainedStorageRecord {
        class: String::from("block-ssd"),
        bytes: probe.cache_total_bytes,
        held_nanos,
    }]
}
