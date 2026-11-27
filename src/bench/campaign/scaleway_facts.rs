//! The repository and command facts a benchmark record pins.
//!
//! These are the parts of the driver that read the local checkout and define
//! what each workload runs. They are separate from measurement so a reader can
//! check what a record will say without following the provider calls.

use camino::Utf8PathBuf;

use super::{PlannedAttempt, scaleway_driver::DriverResult};

/// The worker count every workload is asked to use.
///
/// Exported into each workload's environment so the intended concurrency is a
/// real setting rather than a copy of what the guest happened to provide. A
/// guest with fewer CPUs then shows up as a difference between intended and
/// effective, which is the point of recording both.
pub(super) const INTENDED_WORKERS: u32 = 8;
use crate::bench::{
    observe::RecordedObservation,
    probe::ProbeReport,
    record::{PhaseRecord, PhaseStatus, RecordKind, RunOutcome, SCHEMA_VERSION, TreePin},
};

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
            "else rm -rf target; fi && ",
            "cargo build --workspace --all-targets --all-features"
        ),
        "warm-incremental" => concat!(
            "touch src/lib.rs && ",
            "cargo build --workspace --all-targets --all-features"
        ),
        // Touching the library forces the test binaries to be relinked, so this
        // workload measures linking rather than an up-to-date check.
        "link-heavy" => concat!(
            "touch src/lib.rs && ",
            "cargo test --workspace --all-features --no-run"
        ),
        "test-execution" => concat!(
            "cargo nextest run --workspace --all-targets --all-features ",
            "--test-threads=8"
        ),
        other => {
            return Err(format!(
                "unknown workload {other:?}; the protocol defines cold-build, warm-incremental, \
                 link-heavy and test-execution"
            ));
        }
    };
    // The script is one argument of `sh -c`, and the exported worker count is
    // part of the pinned command vector so a reader can see it.
    let workers = intended_workers();
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
    // A status that could not be read is not an empty status: an empty one
    // would claim a clean tree the harness never established.
    let status = git_output(source, &["status", "--porcelain"])?;
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
    // Split at the authority's last `@`, not its first: a password may contain
    // one, and splitting at the first would leave the password's tail in the
    // recorded URL. The authority ends at the first `/`.
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (rest, None),
    };
    let Some((_credentials, host)) = authority.rsplit_once('@') else {
        return trimmed.to_owned();
    };
    path.map_or_else(
        || format!("{scheme}://{host}"),
        |suffix| format!("{scheme}://{host}/{suffix}"),
    )
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

/// Returns the identifier a run and its phases share within one campaign.
///
/// It includes the provider and product because a campaign compares backends
/// with the same workload and repetition, and those attempts must not share
/// timings.
pub(super) fn run_identifier(campaign_id: &str, attempt: &PlannedAttempt) -> String {
    format!(
        "{campaign_id}/{}/{}/{}/{}",
        attempt.provider, attempt.product, attempt.workload_id, attempt.repetition
    )
}

/// Wraps a workload command in a guest-side deadline.
///
/// `timeout` sends `TERM` at the deadline and `KILL` thirty seconds later, so a
/// workload that ignores the first signal still stops. The wrapper is part of
/// the command the record pins, because it is part of what ran.
pub(super) fn bounded_command(workload: &[String], limit_nanos: u64) -> Vec<String> {
    const NANOS_PER_SECOND: u64 = 1_000_000_000;
    let seconds = limit_nanos
        .checked_div(NANOS_PER_SECOND)
        .unwrap_or(0)
        .max(1);
    let mut command = vec![
        String::from("timeout"),
        String::from("--signal=TERM"),
        String::from("--kill-after=30"),
        seconds.to_string(),
    ];
    command.extend(workload.iter().cloned());
    command
}

/// Converts an attempt's observations into persisted phase records.
///
/// A phase may be observed more than once in one attempt, so each observation
/// takes the next segment number for its phase. The validator then sees a
/// prefix of complete phases and a segment count that matches the observations,
/// rather than a set of records that cannot be distinguished.
pub(super) fn phase_records(
    campaign_id: &str,
    run_id: &str,
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
                run_id: run_id.to_owned(),
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
