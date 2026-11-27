//! The guest-probe commands and the parsers that read them.
//!
//! Each parser reads the exact shape its documented command produces, and a
//! shape that does not match is a typed error rather than a default value: a
//! defaulted CPU count would make a record look measured when it was guessed.

use std::fmt;

use super::{ARCHITECTURE_COMMAND, CPU_COUNT_COMMAND, DISK_COMMAND, MEMORY_COMMAND, truncate};
use crate::bench::record::{Allocation, CacheObservation, CacheState, GuestArchitecture};

/// Prints whether the cache mount path is a mount point, as `0` or `1`.
///
/// `mountpoint -q` reports its answer as an exit status, so the status has to
/// be echoed for a probe that captures standard output to see it.
pub const CACHE_MOUNTED_COMMAND: &str = "mountpoint -q \"$MRIYA_SYNC_VOLUME_MOUNT_PATH\"; echo $?";

/// Prints the number of entries under the cache mount path, excluding the
/// directory the formatter creates.
///
/// The listing is captured before it is counted so a failed `find` prints
/// `unreadable` rather than a zero that would label a populated cache empty.
/// [`parse_unsigned`] rejects that text, so the caller sees a probe failure.
pub const CACHE_ENTRIES_COMMAND: &str = concat!(
    "if entries=$(find \"$MRIYA_SYNC_VOLUME_MOUNT_PATH\" -mindepth 1 -maxdepth 1 ",
    "! -name lost+found); then ",
    "if [ -n \"$entries\" ]; then printf '%s\\n' \"$entries\" | grep -c .; else echo 0; fi; ",
    "else echo unreadable; fi"
);

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
    /// A combined probe report omitted a fact it must carry.
    MissingKey {
        /// The absent fact.
        key: &'static str,
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
            Self::MissingKey { key } => {
                write!(formatter, "probe report omitted the {key} fact")
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
/// # Parameters
///
/// - `outputs`: the raw text of the four allocation probes.
///
/// # Returns
///
/// The measured allocation. `requested_architecture` is empty because the guest
/// cannot know what the provider was asked for; the caller must fill it from
/// the request it sent before persisting the allocation.
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
        // The guest probe cannot know what was requested; the driver fills this
        // from the request it sent.
        requested_architecture: String::new(),
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
/// let absent = cache_from("1\n", "0\n", "0\n")?;
/// assert_eq!(absent.state, CacheState::Absent);
/// assert!(!absent.mounted);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn cache_from(
    mount_output: &str,
    size_output: &str,
    entries_output: &str,
) -> Result<CacheObservation, ProbeError> {
    let mounted = parse_mount_status(mount_output)?;
    let size_bytes = if mounted {
        parse_unsigned(CACHE_SIZE_COMMAND, size_output)?
    } else {
        0
    };
    // A freshly formatted ext4 volume contains `lost+found` and reports used
    // blocks; neither means a previous workload left anything behind. The entry
    // count is what distinguishes an empty cache from a warm one.
    let entries = if mounted {
        parse_unsigned(CACHE_ENTRIES_COMMAND, entries_output)?
    } else {
        0
    };
    Ok(CacheObservation {
        state: cache_state(mounted, entries),
        size_bytes,
        mounted,
        saved_bytes: 0,
    })
}

/// Classifies a mounted cache by whether it already held anything.
///
/// A cache that is not mounted is absent; a mounted empty one is `Empty` rather
/// than `Warm`, so a cold build cannot be mistaken for a warm one.
#[must_use]
pub const fn cache_state(mounted: bool, entries: u64) -> CacheState {
    match (mounted, entries) {
        (false, _) => CacheState::Absent,
        (true, 0) => CacheState::Empty,
        (true, _) => CacheState::Warm,
    }
}
