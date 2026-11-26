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

use std::fmt;

use super::record::{Allocation, CacheObservation, CacheState, Concurrency, GuestArchitecture};

/// Prints the machine architecture, as `uname -m` does.
pub const ARCHITECTURE_COMMAND: &str = "uname -m";

/// Prints the logical processor count.
pub const CPU_COUNT_COMMAND: &str = "getconf _NPROCESSORS_ONLN";

/// Prints total memory in bytes, from `/proc/meminfo`'s `MemTotal` line.
pub const MEMORY_COMMAND: &str = "awk '/^MemTotal:/ {print $2 * 1024}' /proc/meminfo";

/// Prints the workspace filesystem capacity in bytes.
pub const DISK_COMMAND: &str = "df -B1 --output=size \"$PWD\" | tail -n 1";

/// Prints whether the cache mount path is a mount point, as `0` or `1`.
///
/// `mountpoint -q` reports its answer as an exit status, so the status has to
/// be echoed for a probe that captures standard output to see it.
pub const CACHE_MOUNTED_COMMAND: &str = "mountpoint -q \"$MRIYA_SYNC_VOLUME_MOUNT_PATH\"; echo $?";

/// Prints the used bytes of the cache mount path, or `0` when it is absent.
pub const CACHE_SIZE_COMMAND: &str =
    "df -B1 --output=used \"$MRIYA_SYNC_VOLUME_MOUNT_PATH\" 2>/dev/null | tail -n 1 || echo 0";

/// Prints the compiler version banner.
pub const COMPILER_VERSION_COMMAND: &str = "rustc --version";

/// Prints the package manager version banner.
pub const PACKAGE_MANAGER_VERSION_COMMAND: &str = "cargo --version";

/// Prints the `cargo build` JSON message stream for one command.
///
/// The stream is what makes a cache observation measurable: a `fresh` message
/// means Cargo reused an artefact, and a `compiler-artifact` message without
/// one means it rebuilt. Counting the two is how an attempt reports whether its
/// cache actually helped.
pub const CARGO_MESSAGES_SUFFIX: &str = "--message-format=json-render-diagnostics";

/// Why a probe's output could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeError {
    /// A probe printed nothing, dispatch failed, or the command was not found.
    Empty {
        /// The probe that produced no usable output.
        probe: &'static str,
    },
    /// A probe printed something that is not the number it should print.
    NotANumber {
        /// The probe whose output was not a number.
        probe: &'static str,
        /// What it printed, trimmed and truncated for the message.
        found: String,
    },
    /// A probe printed a Cargo JSON message this parser does not understand.
    MalformedMessage {
        /// What the message line contained, truncated for the message.
        found: String,
    },
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { probe } => write!(formatter, "probe {probe} printed nothing usable"),
            Self::NotANumber { probe, found } => {
                write!(
                    formatter,
                    "probe {probe} printed {found:?}, which is not a number"
                )
            }
            Self::MalformedMessage { found } => {
                write!(formatter, "cargo message {found:?} is not a known shape")
            }
        }
    }
}

impl std::error::Error for ProbeError {}

/// Reads a single unsigned integer from a probe's output.
///
/// The first non-blank line is taken, because a login shell may print a banner
/// before the command's own output and the probes are simple one-value tools.
///
/// # Errors
///
/// Returns [`ProbeError::Empty`] when no non-blank line exists, and
/// [`ProbeError::NotANumber`] when the line is not a base-ten integer.
///
/// # Examples
///
/// ```
/// use mriya::bench::probe::parse_unsigned;
///
/// assert_eq!(parse_unsigned("nproc", "8\n")?, 8);
/// assert_eq!(parse_unsigned("nproc", "\n  4  \n")?, 4);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn parse_unsigned(probe: &'static str, output: &str) -> Result<u64, ProbeError> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or(ProbeError::Empty { probe })?;
    line.parse::<u64>().map_err(|_| ProbeError::NotANumber {
        probe,
        found: truncate(line),
    })
}

/// Reads the architecture a guest reported.
///
/// # Errors
///
/// Returns [`ProbeError::Empty`] when the probe printed nothing.
///
/// # Examples
///
/// ```
/// use mriya::bench::{probe::parse_architecture, record::GuestArchitecture};
///
/// assert_eq!(parse_architecture("aarch64\n")?, GuestArchitecture::Aarch64);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn parse_architecture(output: &str) -> Result<GuestArchitecture, ProbeError> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or(ProbeError::Empty {
            probe: ARCHITECTURE_COMMAND,
        })?;
    Ok(GuestArchitecture::from_report(line))
}

/// Reads a `mountpoint -q` exit status echoed by the shell.
///
/// # Errors
///
/// Returns [`ProbeError::Empty`] or [`ProbeError::NotANumber`] when the probe
/// did not print a status.
///
/// # Examples
///
/// ```
/// use mriya::bench::probe::parse_mount_status;
///
/// assert!(!parse_mount_status("1\n")?);
/// assert!(parse_mount_status("0\n")?);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn parse_mount_status(output: &str) -> Result<bool, ProbeError> {
    Ok(parse_unsigned(CACHE_MOUNTED_COMMAND, output)? == 0)
}

/// Builds the allocation a guest reported.
///
/// Every field is measured on the guest, so a product name cannot stand in for
/// what was actually handed over.
///
/// # Errors
///
/// Returns [`ProbeError`] when any probe's output cannot be read.
///
/// # Examples
///
/// ```
/// use mriya::bench::{
///     probe::{ProbeOutputs, allocation_from},
///     record::GuestArchitecture,
/// };
///
/// let observed = allocation_from(&ProbeOutputs {
///     architecture: "x86_64\n",
///     cpu_count: "2\n",
///     memory: "2147483648\n",
///     disk: "21474836480\n",
/// })?;
/// assert_eq!(observed.cpu_count, 2);
/// assert_eq!(observed.guest_architecture, GuestArchitecture::X86_64);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn allocation_from(outputs: &ProbeOutputs<'_>) -> Result<Allocation, ProbeError> {
    Ok(Allocation {
        cpu_count: u32::try_from(parse_unsigned(CPU_COUNT_COMMAND, outputs.cpu_count)?).map_err(
            |_| ProbeError::NotANumber {
                probe: CPU_COUNT_COMMAND,
                found: truncate(outputs.cpu_count),
            },
        )?,
        memory_bytes: parse_unsigned(MEMORY_COMMAND, outputs.memory)?,
        disk_bytes: parse_unsigned(DISK_COMMAND, outputs.disk)?,
        guest_architecture: parse_architecture(outputs.architecture)?,
    })
}

/// The raw text of the four probes that describe an allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeOutputs<'a> {
    /// Output of [`ARCHITECTURE_COMMAND`].
    pub architecture: &'a str,
    /// Output of [`CPU_COUNT_COMMAND`].
    pub cpu_count: &'a str,
    /// Output of [`MEMORY_COMMAND`].
    pub memory: &'a str,
    /// Output of [`DISK_COMMAND`].
    pub disk: &'a str,
}

/// Builds a cache observation from the mount and size probes.
///
/// A cache that is not mounted is reported as [`CacheState::Absent`] whatever
/// the declared intent was, because a record must not claim a cache it did not
/// have.
///
/// # Errors
///
/// Returns [`ProbeError`] when the mount status or size cannot be read.
///
/// # Examples
///
/// ```
/// use mriya::bench::{probe::cache_from, record::CacheState};
///
/// let absent = cache_from("1\n", "0\n")?;
/// assert_eq!(absent.state, CacheState::Absent);
/// assert!(!absent.mounted);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn cache_from(mount_output: &str, size_output: &str) -> Result<CacheObservation, ProbeError> {
    let mounted = parse_mount_status(mount_output)?;
    let size_bytes = if mounted {
        parse_unsigned(CACHE_SIZE_COMMAND, size_output)?
    } else {
        0
    };
    Ok(CacheObservation {
        state: cache_state(mounted, size_bytes),
        size_bytes,
        mounted,
        saved_bytes: 0,
    })
}

/// Classifies a mounted cache by whether it already held anything.
///
/// A cache that is not mounted is absent; a mounted empty one is `Empty` rather
/// than `Warm`, so a cold build cannot be mistaken for a warm one.
fn cache_state(mounted: bool, size_bytes: u64) -> CacheState {
    match (mounted, size_bytes) {
        (false, _) => CacheState::Absent,
        (true, 0) => CacheState::Empty,
        (true, _) => CacheState::Warm,
    }
}

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

/// What one `cargo` JSON message stream reported about cache reuse.
///
/// The two counts are what makes a cache claim checkable. Cargo emits
/// `compiler-artifact` for every unit it produces, marking the ones it reused
/// with `"fresh": true`; counting the marked and unmarked messages gives the
/// rebuilt and reused totals without reading Cargo's human summary, which
/// changes between versions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BuildMessageCounts {
    /// Artefacts Cargo reused without recompiling.
    pub fresh: u32,
    /// Artefacts Cargo rebuilt.
    pub rebuilt: u32,
}

impl BuildMessageCounts {
    /// Returns whether any artefact was reused at all.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::probe::BuildMessageCounts;
    ///
    /// let counts = BuildMessageCounts {
    ///     fresh: 1,
    ///     rebuilt: 0,
    /// };
    /// assert!(counts.reused_anything());
    /// ```
    #[must_use]
    pub const fn reused_anything(&self) -> bool { self.fresh > 0 }

    /// Returns the total number of artefacts Cargo produced or reused.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::probe::BuildMessageCounts;
    ///
    /// assert_eq!(
    ///     BuildMessageCounts {
    ///         fresh: 2,
    ///         rebuilt: 3
    ///     }
    ///     .total(),
    ///     5
    /// );
    /// ```
    #[must_use]
    pub const fn total(&self) -> u32 { self.fresh.saturating_add(self.rebuilt) }
}

/// Counts fresh and rebuilt artefacts in a `cargo` JSON message stream.
///
/// Lines that are not JSON objects, such as the human diagnostics Cargo writes
/// to the same stream, are skipped: the count is about artefacts, and one
/// unparseable diagnostic must not fail a whole build's measurement.
///
/// # Errors
///
/// Returns [`ProbeError::MalformedMessage`] when a line that begins as JSON
/// cannot be read as an object, which would mean the stream is truncated or
/// interleaved rather than merely chatty.
///
/// # Examples
///
/// ```
/// use mriya::bench::probe::count_build_messages;
///
/// let stream = concat!(
///     r#"{"reason":"compiler-artifact","fresh":true}"#,
///     "\n",
///     r#"{"reason":"compiler-artifact"}"#,
///     "\n",
/// );
/// let counts = count_build_messages(stream)?;
/// assert_eq!(counts.fresh, 1);
/// assert_eq!(counts.rebuilt, 1);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn count_build_messages(stream: &str) -> Result<BuildMessageCounts, ProbeError> {
    let mut counts = BuildMessageCounts::default();
    for line in stream
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some(reason) = compiler_artifact_reason(line)? else {
            continue;
        };
        if reason {
            counts.fresh = counts.fresh.saturating_add(1);
        } else {
            counts.rebuilt = counts.rebuilt.saturating_add(1);
        }
    }
    Ok(counts)
}

/// Returns whether `line` is a `compiler-artifact` message, and whether it is
/// marked fresh, or `None` when the line is not such a message.
///
/// # Errors
///
/// Returns [`ProbeError::MalformedMessage`] when a line starting with `{`
/// cannot be parsed as a JSON object.
fn compiler_artifact_reason(line: &str) -> Result<Option<bool>, ProbeError> {
    if !line.starts_with('{') {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|_| ProbeError::MalformedMessage {
            found: truncate(line),
        })?;
    if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
        return Ok(None);
    }
    Ok(Some(
        value
            .get("fresh")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    ))
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
fn truncate(text: &str) -> String {
    const LIMIT: usize = 120;
    let trimmed = text.trim();
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_owned();
    }
    let kept: String = trimmed.chars().take(LIMIT).collect();
    format!("{kept}…")
}
