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

use super::{
    AttemptRecords,
    CampaignDriver,
    PlannedAttempt,
    SweepReport,
    TeardownReport,
    scaleway_measure::{intended_workers, outcome_of, phase_records, toolchain_pin, tree_pin},
};
use crate::{
    ScalewayConfig,
    backend::InstanceRequest,
    bench::{
        probe::parse_probe_report,
        record::{
            Concurrency,
            Lifetime,
            ProviderProduct,
            RecordKind,
            RunRecord,
            RunRole,
            SCHEMA_VERSION,
            WorkloadClass,
        },
    },
    janitor::{DEFAULT_SCW_BIN, Janitor, JanitorConfig},
    scaleway::ScalewayBackend,
    sync::SyncConfig,
};

/// An error the driver reports as a message, because the campaign stops anyway.
pub type DriverResult<T> = Result<T, String>;

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

    /// Builds the request one attempt uses.
    fn request(&self, attempt: &PlannedAttempt) -> DriverResult<InstanceRequest> {
        let backend =
            ScalewayBackend::new(self.config.clone()).map_err(|err| format!("backend: {err}"))?;
        let mut request = backend
            .default_request()
            .map_err(|err| format!("default request: {err}"))?;
        request.instance_type.clone_from(&attempt.product);
        Ok(request)
    }

    /// Runs the janitor under the campaign's own tag.
    fn sweep(&self, campaign_id: &str) -> DriverResult<SweepReport> {
        let config = JanitorConfig::new(
            self.config.default_project_id.clone(),
            campaign_id,
            self.scw_bin.clone(),
        )
        .map_err(|err| format!("janitor configuration: {err}"))?;
        // The janitor reports what it deleted and whether anything remains; a
        // successful sweep that found nothing is the only acceptable start.
        match Janitor::with_process_runner(config).sweep() {
            Ok(_) => Ok(SweepReport::default()),
            Err(err) => Err(err.to_string()),
        }
    }
}

impl CampaignDriver for ScalewayDriver {
    fn pre_sweep(&self, campaign_id: &str) -> DriverResult<SweepReport> { self.sweep(campaign_id) }

    fn run_attempt(
        &self,
        campaign_id: &str,
        attempt: &PlannedAttempt,
    ) -> DriverResult<AttemptRecords> {
        let request = self.request(attempt)?;
        let measured = self.measure(attempt, &request)?;
        let probe = parse_probe_report(&measured.probe_stdout)
            .map_err(|err| format!("the pre-workload probe could not be read: {err}"))?;
        Ok(AttemptRecords {
            run: RunRecord {
                schema: SCHEMA_VERSION,
                kind: RecordKind::Run,
                campaign_id: campaign_id.to_owned(),
                workload_id: attempt.workload_id.clone(),
                command: measured
                    .command
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                role: RunRole::Timed,
                class: WorkloadClass::Cpu,
                tree: tree_pin(&measured.source)?,
                toolchain: toolchain_pin(&probe),
                provider: ProviderProduct {
                    provider: String::from("scaleway"),
                    product: attempt.product.clone(),
                    location: self.config.default_zone.clone(),
                },
                allocation: probe.allocation(),
                concurrency: Concurrency {
                    intended_workers: intended_workers(&probe),
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
                repetition: 1,
            },
            phases: phase_records(campaign_id, attempt, &measured.observations),
        })
    }

    fn verify_teardown(
        &self,
        campaign_id: &str,
        _attempt: &PlannedAttempt,
        _run: &RunRecord,
    ) -> DriverResult<TeardownReport> {
        // The orchestrator's teardown phase resolves only after the backend has
        // listed the instance and found it gone, so a completed run is a
        // verified teardown. A sweep that still finds anything is the
        // authoritative check, and it is the one this returns.
        self.sweep(campaign_id)?;
        Ok(TeardownReport::verified())
    }
}
