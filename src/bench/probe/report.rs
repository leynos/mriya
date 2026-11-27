//! The combined guest probe and the facts it reports.
//!
//! A single command prints every fact a record needs, so a probe costs one
//! remote round trip. The parser reads `key=value` lines rather than a fixed
//! column order, so a probe that fails on a particular host cannot silently
//! shift every later value into the wrong field.

use super::{
    Allocation,
    CacheObservation,
    GuestArchitecture,
    ProbeError,
    cache_state,
    parse_unsigned,
    truncate,
};

/// One shell command that prints every guest fact a record needs.
///
/// A single command keeps the probe cost to one remote round trip, and printing
/// `key=value` lines rather than a fixed column order means a probe that fails
/// on a particular host cannot silently shift every later value into the wrong
/// field. The cache probes tolerate a missing mount path: a backend without a
/// cache still has to produce a record.
pub const PROBE_COMMAND: &str = concat!(
    "printf 'arch=%s\\n' \"$(uname -m)\"; ",
    "printf 'cpu=%s\\n' \"$(getconf _NPROCESSORS_ONLN)\"; ",
    "printf 'mem=%s\\n' \"$(awk '/^MemTotal:/ {print $2 * 1024}' /proc/meminfo)\"; ",
    "printf 'disk=%s\\n' \"$(df -B1 --output=size \"$PWD\" | tail -n 1)\"; ",
    "cache=\"${MRIYA_SYNC_VOLUME_MOUNT_PATH:-/mriya}\"; ",
    "if mountpoint -q \"$cache\" 2>/dev/null; then ",
    "printf 'cache_mounted=1\\n'; ",
    "printf 'cache_bytes=%s\\n' \"$(du -sb \"$cache\" | cut -f1)\"; ",
    "printf 'cache_total_bytes=%s\\n' \"$(df -B1 --output=size \"$cache\" | tail -n 1)\"; ",
    "printf 'cache_entries=%s\\n' \"$(find \"$cache\" -mindepth 1 -maxdepth 1 | wc -l)\"; ",
    "else printf 'cache_mounted=0\\ncache_bytes=0\\ncache_total_bytes=0\\ncache_entries=0\\n'; fi; ",
    "printf 'rustc=%s\\n' \"$(rustc --version 2>/dev/null || echo unknown)\"; ",
    "printf 'cargo=%s\\n' \"$(cargo --version 2>/dev/null || echo unknown)\""
);

/// The guest facts one probe round trip reported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeReport {
    /// Architecture the guest reported.
    pub architecture: GuestArchitecture,
    /// Logical CPUs the guest reported.
    pub cpu_count: u32,
    /// RAM available to the guest, in bytes.
    pub memory_bytes: u64,
    /// Disk available to the workspace, in bytes.
    pub disk_bytes: u64,
    /// Whether the cache path was a mount point.
    pub cache_mounted: bool,
    /// Bytes the cache held.
    pub cache_bytes: u64,
    /// Total size of the cache filesystem, in bytes.
    pub cache_total_bytes: u64,
    /// Entries directly under the cache mount point.
    ///
    /// Used instead of the used-block count to decide whether the cache holds
    /// a previous attempt's artefacts: a freshly formatted filesystem reports
    /// positive usage without holding any.
    pub cache_entries: u64,
    /// The compiler's version banner, or `unknown`.
    pub rustc: String,
    /// The package manager's version banner, or `unknown`.
    pub cargo: String,
}

impl ProbeReport {
    /// Builds the allocation this probe reported.
    #[must_use]
    pub const fn allocation(&self) -> Allocation {
        Allocation {
            // The probe knows only what the guest reported.
            requested_architecture: String::new(),
            cpu_count: self.cpu_count,
            memory_bytes: self.memory_bytes,
            disk_bytes: self.disk_bytes,
            guest_architecture: self.architecture,
        }
    }

    /// Builds the cache observation this probe reported.
    #[must_use]
    pub const fn cache(&self) -> CacheObservation {
        CacheObservation {
            state: cache_state(self.cache_mounted, self.cache_entries),
            size_bytes: if self.cache_mounted {
                self.cache_bytes
            } else {
                0
            },
            mounted: self.cache_mounted,
            saved_bytes: 0,
        }
    }
}

/// Parses the `key=value` lines [`PROBE_COMMAND`] prints.
///
/// # Errors
///
/// Returns [`ProbeError::MissingKey`] naming the first absent fact, so a probe
/// that did not run cannot be mistaken for a guest with no memory.
///
/// # Examples
///
/// ```
/// use mriya::bench::probe::parse_probe_report;
///
/// let report = parse_probe_report(concat!(
///     "arch=x86_64\ncpu=2\nmem=1024\ndisk=2048\n",
///     "cache_mounted=0\ncache_bytes=0\ncache_total_bytes=0\ncache_entries=0\n",
///     "rustc=rustc 1.93.0\ncargo=cargo 1.93.0\n",
/// ))?;
/// assert_eq!(report.cpu_count, 2);
/// assert_eq!(report.rustc, "rustc 1.93.0");
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn parse_probe_report(output: &str) -> Result<ProbeReport, ProbeError> {
    let facts = ProbeFacts::read(output);
    Ok(ProbeReport {
        architecture: GuestArchitecture::from_report(facts.required("arch")?),
        cpu_count: parse_u32("cpu", facts.required("cpu")?)?,
        memory_bytes: parse_unsigned("mem", facts.required("mem")?)?,
        disk_bytes: parse_unsigned("disk", facts.required("disk")?)?,
        cache_mounted: facts.required("cache_mounted")?.trim() == "1",
        cache_bytes: parse_unsigned("cache_bytes", facts.required("cache_bytes")?)?,
        cache_total_bytes: parse_unsigned(
            "cache_total_bytes",
            facts.required("cache_total_bytes")?,
        )?,
        cache_entries: parse_unsigned("cache_entries", facts.required("cache_entries")?)?,
        rustc: facts.required("rustc")?.to_owned(),
        cargo: facts.required("cargo")?.to_owned(),
    })
}

/// The `key=value` facts a probe report carried.
struct ProbeFacts<'a> {
    /// Pairs in the order they appeared.
    pairs: Vec<(&'a str, &'a str)>,
}

impl<'a> ProbeFacts<'a> {
    /// Reads the report's pairs, ignoring lines without a key.
    fn read(output: &'a str) -> Self {
        let pairs = output
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.trim(), value.trim()))
            .collect();
        Self { pairs }
    }

    /// Returns the value for `key`, or names the missing fact.
    fn required(&self, key: &'static str) -> Result<&'a str, ProbeError> {
        self.pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .ok_or(ProbeError::MissingKey { key })
    }
}

/// Reads a `u32` from a probe value, naming the probe and what it printed.
fn parse_u32(probe: &'static str, value: &str) -> Result<u32, ProbeError> {
    value.parse::<u32>().map_err(|_| ProbeError::NotANumber {
        probe,
        found: truncate(value),
    })
}
