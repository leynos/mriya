//! Parsers for the guest probes a benchmark runs before and after a workload.
//!
//! A record must say what was actually allocated, not what the product page
//! promised. These probes ask the guest, and each parser reads the exact shape
//! the documented command produces. A shape that does not match is a typed
//! error rather than a default value, because a defaulted CPU count would make
//! a record look measured when it was guessed.
//!
//! The commands are defined here as constants so the protocol document and the
//! harness cannot drift apart: the harness builds its remote commands from
//! these strings, and the tests parse output that the documented tools really
//! produce.

mod cargo;
mod parsers;
mod report;

pub use cargo::{BuildMessageCounts, count_build_messages};
pub use parsers::{
    CACHE_ENTRIES_COMMAND,
    CACHE_MOUNTED_COMMAND,
    CACHE_SIZE_COMMAND,
    CARGO_MESSAGES_SUFFIX,
    COMPILER_VERSION_COMMAND,
    PACKAGE_MANAGER_VERSION_COMMAND,
    ProbeError,
    ProbeOutputs,
    allocation_from,
    cache_from,
    cache_state,
    parse_architecture,
    parse_mount_status,
    parse_unsigned,
};
pub use report::{PROBE_COMMAND, ProbeReport, parse_probe_report};

use super::record::{Allocation, CacheObservation, Concurrency, GuestArchitecture};

/// Prints the machine architecture, as `uname -m` does.
pub const ARCHITECTURE_COMMAND: &str = "uname -m";

/// Prints the logical processor count.
pub const CPU_COUNT_COMMAND: &str = "getconf _NPROCESSORS_ONLN";

/// Prints total memory in bytes, from `/proc/meminfo`'s `MemTotal` line.
pub const MEMORY_COMMAND: &str = "awk '/^MemTotal:/ {print $2 * 1024}' /proc/meminfo";

/// Prints the workspace filesystem capacity in bytes.
pub const DISK_COMMAND: &str = "df -B1 --output=size \"$PWD\" | tail -n 1";

/// Builds the parallelism a guest provided from the CPU count it reported.
///
/// # Errors
///
/// Returns [`ProbeError`] when the CPU count cannot be read.
///
/// # Examples
///
/// ```
/// use mriya::bench::probe::concurrency_from;
///
/// let observed = concurrency_from(8, "2\n")?;
/// assert_eq!(observed.intended_workers, 8);
/// assert_eq!(observed.effective_workers, 2);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn concurrency_from(
    intended_workers: u32,
    cpu_output: &str,
) -> Result<Concurrency, ProbeError> {
    let effective =
        u32::try_from(parse_unsigned(CPU_COUNT_COMMAND, cpu_output)?).map_err(|_| {
            ProbeError::NotANumber {
                probe: CPU_COUNT_COMMAND,
                found: truncate(cpu_output),
            }
        })?;
    Ok(Concurrency {
        intended_workers,
        effective_workers: effective,
    })
}

/// Returns a single version banner from a tool's output.
///
/// The banner is kept verbatim because it names both the tool and its version,
/// and rewriting it would lose the distinction between a stable release and a
/// nightly of the same number.
///
/// # Errors
///
/// Returns [`ProbeError::Empty`] when the tool printed nothing.
///
/// # Examples
///
/// ```
/// use mriya::bench::probe::parse_version_banner;
///
/// let banner = parse_version_banner("rustc 1.93.0-nightly (abc 2026-11-01)\n")?;
/// assert_eq!(banner, "rustc 1.93.0-nightly (abc 2026-11-01)");
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn parse_version_banner(output: &str) -> Result<String, ProbeError> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
        .ok_or(ProbeError::Empty {
            probe: COMPILER_VERSION_COMMAND,
        })
}

/// Splits a version banner into the tool name and its version.
///
/// # Errors
///
/// Returns [`ProbeError::Empty`] when the banner names no tool.
///
/// # Examples
///
/// ```
/// use mriya::bench::probe::split_banner;
///
/// assert_eq!(
///     split_banner("cargo 1.93.0-nightly (abc 2026-11-01)")?,
///     (
///         "cargo".to_owned(),
///         "1.93.0-nightly (abc 2026-11-01)".to_owned()
///     )
/// );
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn split_banner(banner: &str) -> Result<(String, String), ProbeError> {
    let mut parts = banner.trim().splitn(2, char::is_whitespace);
    let tool = parts.next().unwrap_or_default();
    if tool.is_empty() {
        return Err(ProbeError::Empty {
            probe: COMPILER_VERSION_COMMAND,
        });
    }
    let version = parts.next().unwrap_or_default().trim().to_owned();
    Ok((tool.to_owned(), version))
}

/// Shortens `text` for an error message so a whole JSON line cannot flood it.
pub(super) fn truncate(text: &str) -> String {
    const LIMIT: usize = 120;
    let trimmed = text.trim();
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_owned();
    }
    let kept: String = trimmed.chars().take(LIMIT).collect();
    format!("{kept}…")
}
