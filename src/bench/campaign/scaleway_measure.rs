//! The measurement half of the Scaleway driver.
//!
//! This module runs one attempt and reports what it measured. It deliberately
//! does not build a record: turning a measurement into a record is a separate
//! job with separate rules, and keeping them apart makes each testable on its
//! own.

use std::sync::Arc;

use camino::Utf8PathBuf;

use super::{PlannedAttempt, ScalewayDriver, scaleway_driver::DriverResult};
use crate::{
    RunOrchestrator,
    backend::InstanceRequest,
    bench::{
        lifecycle::RecordingSink,
        observe::{Observation, RecordedObservation, RecordingObserver, RunObserver},
        probe::{PROBE_COMMAND, ProbeReport},
        record::{PhaseRecord, PhaseStatus, RecordKind, RunOutcome, SCHEMA_VERSION, TreePin},
    },
    scaleway::ScalewayBackend,
    sync::{StreamingCommandRunner, Syncer},
};

/// Everything one attempt measured, before it is turned into a record.
pub(super) struct Measurement {
    /// The workspace the attempt ran in.
    pub(super) source: Utf8PathBuf,
    /// The command the workload ran.
    pub(super) command: String,
    /// The attempt's wall-clock lifetime.
    pub(super) total_nanos: u64,
    /// Standard output of the pre-workload probe.
    pub(super) probe_stdout: String,
    /// The workload's own output.
    pub(super) output: crate::sync::RemoteCommandOutput,
    /// Every phase observation, in order.
    pub(super) observations: Vec<RecordedObservation>,
}

/// Returns the current working directory as a UTF-8 path.
pub(super) fn current_directory() -> DriverResult<Utf8PathBuf> {
    let directory = std::env::current_dir().map_err(|err| format!("working directory: {err}"))?;
    Utf8PathBuf::from_path_buf(directory)
        .map_err(|bad| format!("working directory is not UTF-8: {}", bad.display()))
}

/// Converts an attempt's observations into persisted phase records.
///
/// A phase may be observed more than once in one attempt, so each observation
/// takes the next segment number for its phase. The validator then sees a
/// prefix of complete phases and a segment count that matches the observations,
/// rather than a set of records that cannot be distinguished.
pub(super) fn phase_records(
    campaign_id: &str,
    attempt: &PlannedAttempt,
    observations: &[RecordedObservation],
) -> Vec<PhaseRecord> {
    let mut segments: std::collections::BTreeMap<crate::bench::record::PhaseKind, u32> =
        std::collections::BTreeMap::new();
    observations
        .iter()
        .filter_map(|observation| {
            let (phase, duration_nanos, status) = match observation {
                RecordedObservation::Started(_) => return None,
                RecordedObservation::Finished {
                    phase,
                    duration_nanos,
                    succeeded,
                } => (
                    *phase,
                    *duration_nanos,
                    if *succeeded {
                        PhaseStatus::Ok
                    } else {
                        PhaseStatus::Failed
                    },
                ),
                RecordedObservation::Unverified {
                    phase,
                    duration_nanos,
                } => (*phase, *duration_nanos, PhaseStatus::Unverified),
            };
            let segment = segments.entry(phase).or_insert(0);
            *segment = segment.saturating_add(1);
            Some(PhaseRecord {
                schema: SCHEMA_VERSION,
                kind: RecordKind::Phase,
                campaign_id: campaign_id.to_owned(),
                workload_id: attempt.workload_id.clone(),
                repetition: 1,
                phase,
                segment: *segment,
                duration_nanos,
                status,
            })
        })
        .collect()
}

/// Adapts an `Arc<RecordingObserver>` to the orchestrator's boxed observer.
pub(super) struct SharedObserver(Arc<RecordingObserver>);

impl RunObserver for SharedObserver {
    fn observe(&self, observation: Observation<'_>) { self.0.observe(observation); }
}

/// Returns the workload command the protocol pins for `workload_id`.
pub(super) fn workload_command(workload_id: &str) -> DriverResult<String> {
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
pub(super) fn outcome_of(output: &crate::sync::RemoteCommandOutput) -> RunOutcome {
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
pub(super) fn toolchain_pin(probe: &ProbeReport) -> crate::bench::record::ToolchainPin {
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
pub(super) fn banner_parts(banner: &str) -> (String, String) {
    match banner.split_once(char::is_whitespace) {
        Some((tool, version)) if !tool.is_empty() => (tool.to_owned(), version.trim().to_owned()),
        _ => (String::from("unknown"), banner.trim().to_owned()),
    }
}

/// Returns the worker count the workload was asked to use.
///
/// The protocol pins an intended worker count, and the Cargo workloads here all
/// honour `CARGO_BUILD_JOBS` through the environment the orchestration sets.
pub(super) const fn intended_workers(probe: &ProbeReport) -> u32 { probe.cpu_count }

/// Builds the tree pin from the repository the campaign runs in.
///
/// # Errors
///
/// Returns a message when the repository state cannot be read, because a record
/// that guessed its revision would be evidence of nothing.
pub(super) fn tree_pin(source: &Utf8PathBuf) -> DriverResult<TreePin> {
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
pub(super) fn git_output(source: &Utf8PathBuf, args: &[&str]) -> DriverResult<String> {
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
pub(super) fn status_signature(status: &str) -> String {
    let mut hash = 0_u64;
    for byte in status.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(byte));
    }
    format!("status-fnv64:{hash:016x}")
}

/// Converts a `Duration` into nanoseconds without a lossy cast.
pub(super) fn elapsed_nanos(elapsed: std::time::Duration) -> u64 {
    u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
}

impl ScalewayDriver {
    /// Executes one attempt and returns everything it measured.
    ///
    /// Kept separate from record construction so each half reads as one job:
    /// this half runs the workload, and `run_attempt` turns the result into a
    /// record.
    pub(super) fn measure(
        &self,
        attempt: &PlannedAttempt,
        request: &InstanceRequest,
    ) -> DriverResult<Measurement> {
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

        let source = current_directory()?;
        let command = workload_command(&attempt.workload_id)?;
        let runtime =
            tokio::runtime::Runtime::new().map_err(|err| format!("async runtime: {err}"))?;
        let started = std::time::Instant::now();
        let attempt_output = runtime
            .block_on(async {
                orchestrator
                    .execute_observed(request, &source, &command)
                    .await
            })
            .map_err(|err| format!("run failed: {err}"))?;
        let total_nanos = elapsed_nanos(started.elapsed());
        let probe_stdout = attempt_output
            .pre_stdout
            .ok_or_else(|| String::from("the pre-workload probe produced no output"))?;
        Ok(Measurement {
            source,
            command,
            total_nanos,
            probe_stdout,
            output: attempt_output.output,
            observations: observer.observations(),
        })
    }
}
