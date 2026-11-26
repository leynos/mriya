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
    sync::{StreamingCommandRunner, Syncer},
};

/// Everything one attempt measured, before it is turned into a record.
pub(super) struct Measurement {
    /// The workspace the attempt ran in.
    pub(super) source: Utf8PathBuf,
    /// The command vector the workload ran.
    pub(super) command: Vec<String>,
    /// The attempt's wall-clock lifetime.
    pub(super) total_nanos: u64,
    /// Standard output of the pre-workload probe.
    pub(super) probe_stdout: String,
    /// The workload's own output.
    pub(super) output: crate::sync::RemoteCommandOutput,
    /// Every phase observation, in order.
    pub(super) observations: Vec<RecordedObservation>,
    /// Provider identifiers the attempt created.
    pub(super) resource_ids: Vec<String>,
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
                repetition: attempt.repetition,
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

/// The worker count every workload is asked to use.
///
/// Exported into each workload's environment so the intended concurrency is a
/// real setting rather than a copy of what the guest happened to provide. A
/// guest with fewer CPUs then shows up as a difference between intended and
/// effective, which is the point of recording both.
pub(super) const INTENDED_WORKERS: u32 = 8;

/// Returns the workload command the protocol pins for `workload_id`, as the
/// exact argument vector the record carries.
///
/// Each workload does what its identifier claims:
///
/// - `cold-build` clears the routed target directory before building, so it cannot silently reuse a
///   cache.
/// - `warm-incremental` touches one source file before building, so it measures a real recompile
///   rather than a no-op.
/// - `link-heavy` links every test binary without running it.
/// - `test-execution` runs the repository's test command over the warm build.
///
/// # Errors
///
/// Returns a message naming the workload when the identifier is not one the
/// protocol defines, rather than running a command that does not match the
/// record.
pub(super) fn workload_command(workload_id: &str) -> DriverResult<Vec<String>> {
    let mut cargo = String::from("cargo");
    let script = match workload_id {
        "cold-build" => concat!(
            "if [ -n \"$CARGO_TARGET_DIR\" ]; then rm -rf \"$CARGO_TARGET_DIR\"; ",
            "else rm -rf target; fi; cargo build --workspace --all-targets --all-features"
        ),
        "warm-incremental" => concat!(
            "touch src/lib.rs; ",
            "cargo build --workspace --all-targets --all-features"
        ),
        "link-heavy" => "cargo test --workspace --all-features --no-run",
        "test-execution" => "cargo nextest run --workspace --all-targets --all-features",
        other => {
            return Err(format!(
                "unknown workload {other:?}; the protocol defines cold-build, warm-incremental, \
                 link-heavy and test-execution"
            ));
        }
    };
    // The script is one argument of `sh -c`, and the exported worker count is
    // part of the pinned command vector so a reader can see it.
    let workers = INTENDED_WORKERS;
    let exported = format!("export CARGO_BUILD_JOBS={workers}; {script}");
    cargo.clear();
    Ok(vec![String::from("sh"), String::from("-c"), exported])
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

/// Returns the worker count every workload was asked to use.
pub(super) const fn intended_workers() -> u32 { INTENDED_WORKERS }

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
    let files = git_output(source, &["ls-tree", "-r", "-l", "HEAD"])?;
    let (file_count, size_bytes) = tree_size(&files);
    Ok(TreePin {
        remote: redact_remote(&remote),
        commit,
        tree_digest: status_signature(&status),
        dirty: !status.trim().is_empty(),
        size_bytes,
        file_count,
    })
}

/// Returns the file count and total byte size of a `git ls-tree -r -l` listing.
///
/// The listing's size column is the fourth whitespace-separated field, so a
/// path containing spaces does not disturb the sum: only the leading fields are
/// read.
pub(crate) fn tree_size(listing: &str) -> (u64, u64) {
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    for line in listing.lines().filter(|line| !line.trim().is_empty()) {
        count = count.saturating_add(1);
        if let Some(size) = line
            .split_whitespace()
            .nth(3)
            .and_then(|size| size.parse::<u64>().ok())
        {
            bytes = bytes.saturating_add(size);
        }
    }
    (count, bytes)
}

/// Removes any userinfo from a repository URL before it is persisted.
///
/// A remote configured as `https://user:token@host/path` would otherwise write
/// the token into every record. The host and path are kept, because they are
/// what identifies the repository; everything before the `@` is dropped.
pub(crate) fn redact_remote(remote: &str) -> String {
    let trimmed = remote.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_owned();
    };
    // In an HTTP(S) URL the userinfo is a credential. In an SSH URL it is the
    // login name, which identifies the repository's owner rather than
    // authenticating the caller, so it is kept.
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return trimmed.to_owned();
    }
    match rest.split_once('@') {
        Some((_credentials, host_and_path)) => format!("{scheme}://{host_and_path}"),
        None => trimmed.to_owned(),
    }
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
        campaign_id: &str,
        attempt: &PlannedAttempt,
        request: &InstanceRequest,
    ) -> Result<Measurement, super::ports::AttemptFailure> {
        let lifecycle = Arc::new(RecordingSink::default());
        let backend = self
            .backend(campaign_id, Some(lifecycle.clone()))
            .map_err(super::ports::AttemptFailure::without_resources)?;
        let syncer =
            Syncer::new(self.sync_config.clone(), StreamingCommandRunner).map_err(|err| {
                super::ports::AttemptFailure::without_resources(format!("syncer: {err}"))
            })?;
        let observer = Arc::new(RecordingObserver::default());
        let orchestrator = RunOrchestrator::new(backend, syncer)
            .with_observer(Box::new(SharedObserver(observer.clone())))
            .with_pre_workload_command(PROBE_COMMAND)
            .with_post_workload_command(PROBE_COMMAND);

        let source =
            current_directory().map_err(super::ports::AttemptFailure::without_resources)?;
        let command = workload_command(&attempt.workload_id)
            .map_err(super::ports::AttemptFailure::without_resources)?;
        let rendered = shell_escape(&command);
        let runtime = tokio::runtime::Runtime::new().map_err(|err| {
            super::ports::AttemptFailure::without_resources(format!("async runtime: {err}"))
        })?;
        let started = std::time::Instant::now();
        let attempt_output = runtime
            .block_on(async {
                orchestrator
                    .execute_observed(request, &source, &rendered)
                    .await
            })
            .map_err(|err| super::ports::AttemptFailure {
                reason: format!("run failed: {err}"),
                resource_ids: lifecycle.created_ids(),
            })?;
        let total_nanos = elapsed_nanos(started.elapsed());
        // The run completed and its teardown was verified, so a later failure
        // leaves nothing behind and is reported without resource identifiers.
        let probe_stdout = attempt_output.pre_stdout.ok_or_else(|| {
            super::ports::AttemptFailure::without_resources(
                "the pre-workload probe produced no output",
            )
        })?;
        Ok(Measurement {
            source,
            command,
            total_nanos,
            probe_stdout,
            output: attempt_output.output,
            observations: observer.observations(),
            resource_ids: lifecycle.created_ids(),
        })
    }
}

/// Renders a command vector as one shell command line.
///
/// The orchestrator takes a string, and each argument is escaped so the remote
/// shell sees the same words the record pins.
pub(crate) fn shell_escape(command: &[String]) -> String {
    command
        .iter()
        .map(|argument| shell_escape::unix::escape(argument.as_str().into()).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}
