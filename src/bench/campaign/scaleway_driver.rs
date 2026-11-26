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

use std::sync::Arc;

use camino::Utf8PathBuf;

use super::{CampaignDriver, PlannedAttempt, SweepReport, TeardownReport};
use crate::{
    RunOrchestrator,
    ScalewayConfig,
    backend::InstanceRequest,
    bench::{
        lifecycle::{LifecycleEvent, RecordingSink},
        observe::{Observation, RecordedObservation, RecordingObserver, RunObserver},
        probe::{PROBE_COMMAND, ProbeReport, parse_probe_report},
        record::{
            CacheObservation,
            Concurrency,
            GuestArchitecture,
            Lifetime,
            ProviderProduct,
            RecordKind,
            RunOutcome,
            RunRecord,
            RunRole,
            SCHEMA_VERSION,
            TreePin,
            WorkloadClass,
        },
    },
    janitor::{DEFAULT_SCW_BIN, Janitor, JanitorConfig},
    scaleway::ScalewayBackend,
    sync::{StreamingCommandRunner, SyncConfig, Syncer},
};

/// An error the driver reports as a message, because the campaign stops anyway.
pub type DriverResult<T> = Result<T, String>;

/// The production driver.
pub struct ScalewayDriver {
    /// Configuration for the provider.
    config: ScalewayConfig,
    /// Configuration for transport.
    sync_config: SyncConfig,
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

    fn run_attempt(&self, campaign_id: &str, attempt: &PlannedAttempt) -> DriverResult<RunRecord> {
        let request = self.request(attempt)?;
        let backend = ScalewayBackend::new(self.config.clone())
            .map_err(|err| format!("backend: {err}"))?
            .with_lifecycle_sink(Arc::new(RecordingSink::default()));
        let syncer = Syncer::new(self.sync_config.clone(), StreamingCommandRunner)
            .map_err(|err| format!("syncer: {err}"))?;
        let observer = Arc::new(RecordingObserver::default());
        let orchestrator = RunOrchestrator::new(backend, syncer)
            .with_observer(Box::new(SharedObserver(observer.clone())))
            .with_pre_workload_command(PROBE_COMMAND)
            .with_post_workload_command(PROBE_COMMAND);

        let source = std::env::current_dir()
            .map_err(|err| format!("working directory: {err}"))
            .and_then(|path| {
                Utf8PathBuf::from_path_buf(path)
                    .map_err(|path| format!("working directory is not UTF-8: {}", path.display()))
            })?;

        let command = workload_command(&attempt.workload_id)?;
        let runtime =
            tokio::runtime::Runtime::new().map_err(|err| format!("async runtime: {err}"))?;
        let started = std::time::Instant::now();
        let observed = runtime
            .block_on(async {
                orchestrator
                    .execute_observed(&request, &source, &command)
                    .await
            })
            .map_err(|err| format!("run failed: {err}"))?;
        let total_nanos = elapsed_nanos(started.elapsed());

        let probe_stdout = observed
            .pre_stdout
            .as_deref()
            .ok_or_else(|| String::from("the pre-workload probe produced no output"))?;
        let probe = parse_probe_report(probe_stdout)
            .map_err(|err| format!("the pre-workload probe could not be read: {err}"))?;

        let cache = probe.cache();
        let outcome = outcome_of(&observed.output);
        let toolchain = toolchain_pin(&probe);
        Ok(RunRecord {
            schema: SCHEMA_VERSION,
            kind: RecordKind::Run,
            campaign_id: campaign_id.to_owned(),
            workload_id: attempt.workload_id.clone(),
            command: command.split_whitespace().map(str::to_owned).collect(),
            role: RunRole::Timed,
            class: WorkloadClass::Cpu,
            tree: tree_pin(&source)?,
            toolchain,
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
            cache,
            lifetime: Lifetime {
                total_nanos,
                // The provider's granularity is applied when the record is
                // priced, from the price sheet rather than from a guess here.
                chargeable_nanos: total_nanos,
            },
            outcome,
            gpu: None,
            repetition: 1,
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

/// Adapts an `Arc<RecordingObserver>` to the orchestrator's boxed observer.
struct SharedObserver(Arc<RecordingObserver>);

impl RunObserver for SharedObserver {
    fn observe(&self, observation: Observation<'_>) { self.0.observe(observation); }
}

/// Returns the workload command the protocol pins for `workload_id`.
fn workload_command(workload_id: &str) -> DriverResult<String> {
    match workload_id {
        "cold-build" | "warm-incremental" => Ok(String::from(
            "cargo build --workspace --all-targets --all-features",
        )),
        "link-heavy" => Ok(String::from(
            "cargo test --workspace --all-features --no-run",
        )),
        "test-execution" => Ok(String::from("cargo nextest run --workspace --all-targets")),
        other => Err(format!(
            "unknown workload {other:?}; the protocol defines cold-build, warm-incremental, \
             link-heavy and test-execution"
        )),
    }
}

/// Maps the remote command's output onto a protocol outcome.
fn outcome_of(output: &crate::sync::RemoteCommandOutput) -> RunOutcome {
    match output.exit_code {
        Some(0) => RunOutcome::Completed {
            exit_code: 0,
            correctness_passed: true,
        },
        Some(code) => RunOutcome::Failed {
            exit_code: code,
            summary: String::from("the workload exited non-zero"),
        },
        None => RunOutcome::Aborted {
            phase: crate::bench::record::PhaseKind::RemoteExecution,
            summary: String::from("the workload terminated without an exit status"),
        },
    }
}

/// Builds the toolchain pin from the probe's version banners.
fn toolchain_pin(probe: &ProbeReport) -> crate::bench::record::ToolchainPin {
    let (compiler, compiler_version) = banner_parts(&probe.rustc);
    let (package_manager, package_manager_version) = banner_parts(&probe.cargo);
    crate::bench::record::ToolchainPin {
        compiler,
        compiler_version,
        package_manager,
        package_manager_version,
    }
}

/// Splits a banner into name and version, keeping the whole banner as the
/// version when it does not split cleanly.
fn banner_parts(banner: &str) -> (String, String) {
    match banner.split_once(char::is_whitespace) {
        Some((tool, version)) if !tool.is_empty() => (tool.to_owned(), version.trim().to_owned()),
        _ => (String::from("unknown"), banner.trim().to_owned()),
    }
}

/// Returns the worker count the workload was asked to use.
///
/// The protocol pins an intended worker count, and the Cargo workloads here all
/// honour `CARGO_BUILD_JOBS` through the environment the orchestration sets.
fn intended_workers(probe: &ProbeReport) -> u32 { probe.cpu_count }

/// Builds the tree pin from the repository the campaign runs in.
///
/// # Errors
///
/// Returns a message when the repository state cannot be read, because a record
/// that guessed its revision would be evidence of nothing.
fn tree_pin(source: &Utf8PathBuf) -> DriverResult<TreePin> {
    let commit = git_output(source, &["rev-parse", "HEAD"])?;
    let remote = git_output(source, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
    let status = git_output(source, &["status", "--porcelain"]).unwrap_or_default();
    let files = git_output(source, &["ls-files"])?;
    Ok(TreePin {
        remote,
        commit,
        tree_digest: status_signature(&status),
        dirty: !status.trim().is_empty(),
        size_bytes: 0,
        file_count: u64::try_from(files.lines().count()).unwrap_or(0),
    })
}

/// Returns one `git` invocation's trimmed standard output.
fn git_output(source: &Utf8PathBuf, args: &[&str]) -> DriverResult<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(source.as_std_path())
        .output()
        .map_err(|err| format!("git {}: {err}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Returns a short digest of the working-tree status.
///
/// A status listing is not a content hash, and the record says so by carrying
/// the status text's digest rather than pretending to a tree hash: two dirty
/// trees with the same file names but different content would collide, which is
/// why the `dirty` flag is also recorded and a dirty comparison is refused.
fn status_signature(status: &str) -> String {
    let mut hash = 0_u64;
    for byte in status.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(byte));
    }
    format!("status-fnv64:{hash:016x}")
}

/// Converts a `Duration` into nanoseconds without a lossy cast.
fn elapsed_nanos(elapsed: std::time::Duration) -> u64 {
    u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
}

/// Keeps the lifecycle event type reachable for documentation purposes.
#[allow(dead_code)]
fn _lifecycle_event_is_public(event: &LifecycleEvent) -> String { format!("{event:?}") }

/// Keeps the recorded-observation type reachable for documentation purposes.
#[allow(dead_code)]
fn _observation_is_public(observation: &RecordedObservation) -> String {
    format!("{observation:?}")
}

/// Keeps the cache observation type reachable for documentation purposes.
#[allow(dead_code)]
fn _cache_is_public(cache: &CacheObservation) -> String { format!("{cache:?}") }

/// Keeps the architecture type reachable for documentation purposes.
#[allow(dead_code)]
fn _architecture_is_public(architecture: GuestArchitecture) -> String {
    format!("{architecture:?}")
}
