//! The pinned facts every benchmark record carries.
//!
//! Each type here answers one question the protocol says a reader must be able
//! to answer from the record alone: what code ran, on what hardware, with what
//! cache, and with how much parallelism. Keeping them in their own module lets
//! [`crate::bench::validate`] name a missing pin precisely — `allocation.cpu_count`
//! rather than "the run record is incomplete".

use serde::{Deserialize, Serialize};

/// The source tree a workload ran against.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TreePin {
    /// Repository remote the tree came from.
    pub remote: String,
    /// Commit identifier checked out for the run.
    pub commit: String,
    /// Digest of the synchronized tree, so two trees can be compared.
    pub tree_digest: String,
    /// Whether the working tree had uncommitted changes.
    pub dirty: bool,
    /// Bytes transferred for the workspace.
    pub size_bytes: u64,
    /// Files transferred for the workspace.
    pub file_count: u64,
}

/// The resources the provider actually allocated.
///
/// These are measured from the guest, not copied from the provider's product
/// page, because a product name does not determine what was handed over.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Allocation {
    /// Logical CPUs the guest reported.
    pub cpu_count: u32,
    /// RAM available to the guest, in bytes.
    pub memory_bytes: u64,
    /// Disk available to the workspace, in bytes.
    pub disk_bytes: u64,
    /// Architecture the guest reported, for example `x86_64`.
    pub guest_architecture: GuestArchitecture,
}

/// The architecture a guest reported for itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuestArchitecture {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
    /// Anything the guest reported that this version does not name.
    Other,
}

impl GuestArchitecture {
    /// Maps a guest-reported architecture string onto a known variant.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::record::GuestArchitecture;
    ///
    /// assert_eq!(
    ///     GuestArchitecture::from_report("x86_64"),
    ///     GuestArchitecture::X86_64
    /// );
    /// assert_eq!(
    ///     GuestArchitecture::from_report("aarch64"),
    ///     GuestArchitecture::Aarch64
    /// );
    /// assert_eq!(
    ///     GuestArchitecture::from_report("riscv64"),
    ///     GuestArchitecture::Other
    /// );
    /// ```
    #[must_use]
    pub fn from_report(reported: &str) -> Self {
        match reported.trim() {
            "x86_64" | "amd64" => Self::X86_64,
            "aarch64" | "arm64" => Self::Aarch64,
            _ => Self::Other,
        }
    }
}

/// Declared and effective parallelism.
///
/// A build system that ignores its worker setting still records the setting, so
/// the report can show the difference rather than crediting the backend for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Concurrency {
    /// Worker count the operator asked the workload to use.
    pub intended_workers: u32,
    /// Parallelism the guest actually provided, as measured on the guest.
    pub effective_workers: u32,
}

/// The cache state an attempt started from.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheState {
    /// The cache location held nothing.
    Empty,
    /// The cache location held a previous attempt's artefacts.
    Warm,
    /// The cache location was populated from a declared seed artefact.
    Seed,
    /// The backend cannot persist a cache at all.
    Absent,
}

/// What the attempt observed about its cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CacheObservation {
    /// The cache class the attempt started from.
    pub state: CacheState,
    /// Bytes the cache held when the attempt started.
    pub size_bytes: u64,
    /// Whether the cache location was confirmed mounted for this attempt.
    pub mounted: bool,
    /// Bytes written back to the cache when the attempt finished.
    pub saved_bytes: u64,
}

/// The provider and product a run used.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderProduct {
    /// Provider name, for example `scaleway`.
    pub provider: String,
    /// The product or resource class actually allocated.
    pub product: String,
    /// The location or zone the resource was allocated in.
    pub location: String,
}

/// Accelerator details, present exactly on a GPU experiment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GpuSpec {
    /// Accelerator model name.
    pub model: String,
    /// Number of accelerators attached.
    pub count: u32,
    /// Memory per accelerator, in bytes.
    pub memory_bytes_per_device: u64,
    /// Driver version the guest reported.
    pub driver_version: String,
    /// Runtime version the workload used.
    pub runtime_version: String,
    /// Whether the workload's own correctness check passed.
    pub correctness_passed: bool,
}
