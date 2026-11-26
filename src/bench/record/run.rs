//! The per-attempt records: one run and the phases it reported.
//!
//! A run record describes one attempt at one workload, and a phase record
//! describes one span within it. They are separate lines so a partially written
//! file still yields the phases that completed, and so a reader can price a run
//! without parsing every phase.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{CacheObservation, Concurrency, GpuSpec, ProviderProduct, RecordKind, TreePin};

/// One attempt at one workload on one backend.
///
/// The record holds the pins, the measured lifetime, and the outcome. It holds
/// no phase timings: those are separate phase records so a partially written
/// file still yields the phases that completed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// Schema version this record was written with.
    pub schema: u32,
    /// Always [`RecordKind::Run`].
    pub kind: RecordKind,
    /// Campaign this attempt belongs to.
    pub campaign_id: String,
    /// Workload identifier from the protocol's workload set.
    pub workload_id: String,
    /// The exact command vector, so two attempts can be compared honestly.
    pub command: Vec<String>,
    /// Declared role: a timed attempt or a cache warm-up.
    pub role: RunRole,
    /// Whether this is a CPU or GPU experiment.
    pub class: WorkloadClass,
    /// What the workload ran against.
    pub tree: TreePin,
    /// The toolchain that performed the work.
    pub toolchain: ToolchainPin,
    /// The provider and product actually used.
    pub provider: ProviderProduct,
    /// The resources actually allocated.
    pub allocation: super::Allocation,
    /// Declared and effective parallelism.
    pub concurrency: Concurrency,
    /// The cache state the attempt started from.
    pub cache: CacheObservation,
    /// The measured chargeable lifetime.
    pub lifetime: Lifetime,
    /// How the attempt ended.
    pub outcome: RunOutcome,
    /// GPU details, present exactly when `class` is [`WorkloadClass::Gpu`].
    pub gpu: Option<GpuSpec>,
    /// Identifier of the repetition within the workload, starting at one.
    pub repetition: u32,
}

/// Which part of the protocol a run serves.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunRole {
    /// A measured attempt, included in the timing summary.
    #[default]
    Timed,
    /// A cache warm-up, recorded for cost but excluded from timing.
    WarmUp,
}

/// Whether an experiment measured a processor or an accelerator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkloadClass {
    /// Central processing unit work.
    Cpu,
    /// Graphics processing unit work, ranked separately.
    Gpu,
}

/// The compiler, runtime, and package manager that performed the work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolchainPin {
    /// Compiler or runtime name, for example `rustc`.
    pub compiler: String,
    /// Compiler or runtime version string.
    pub compiler_version: String,
    /// Package manager name, for example `cargo`.
    pub package_manager: String,
    /// Package manager version string.
    pub package_manager_version: String,
}

/// A measured span of time in nanoseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Lifetime {
    /// Total wall-clock time from create request to verified teardown.
    pub total_nanos: u64,
    /// Billable lifetime the writer recorded for this attempt.
    ///
    /// A writer that knows the provider's granularity may already have applied
    /// it. The report prices from this figure and lets the sheet round again,
    /// which is idempotent, so a record that applied a minimum is never
    /// re-rounded downwards. The validator requires it to be at least the
    /// measured lifetime.
    pub chargeable_nanos: u64,
}

/// How an attempt ended.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum RunOutcome {
    /// The command completed with the recorded status.
    Completed {
        /// Exit status the remote command reported.
        exit_code: i32,
        /// Whether the remote command's own check considered the work correct.
        correctness_passed: bool,
    },
    /// The command started and failed.
    Failed {
        /// Exit status the remote command reported.
        exit_code: i32,
        /// Short, credential-free description of the failure.
        summary: String,
    },
    /// The attempt never reached the remote command.
    Aborted {
        /// Which phase ended the attempt.
        phase: PhaseKind,
        /// Short, credential-free description of the failure.
        summary: String,
    },
}

/// The phases of one attempt, in the order the protocol defines them.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhaseKind {
    /// Issuing the create request and receiving a resource identity.
    Allocate,
    /// Waiting for the guest to be ready for remote execution.
    Bootstrap,
    /// Attaching or restoring the cache.
    CacheRestore,
    /// Transferring the workspace.
    Sync,
    /// Running the remote command.
    RemoteExecution,
    /// Persisting the cache.
    CacheSave,
    /// Deleting the resource and verifying it is gone.
    Teardown,
}

impl PhaseKind {
    /// The phases in protocol order, which is also execution order.
    pub const ALL: [Self; 7] = [
        Self::Allocate,
        Self::Bootstrap,
        Self::CacheRestore,
        Self::Sync,
        Self::RemoteExecution,
        Self::CacheSave,
        Self::Teardown,
    ];

    /// Returns the phase name as written to the file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allocate => "allocate",
            Self::Bootstrap => "bootstrap",
            Self::CacheRestore => "cache-restore",
            Self::Sync => "sync",
            Self::RemoteExecution => "remote-execution",
            Self::CacheSave => "cache-save",
            Self::Teardown => "teardown",
        }
    }
}

impl fmt::Display for PhaseKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One phase of one attempt.
///
/// A phase may be observed more than once in one attempt. Bootstrap is the
/// case that matters: the guest becomes reachable at one point, and a
/// cloud-init wait extends the same "still becoming usable" period later,
/// after the workspace has been synced. Rather than move either wait into a
/// phase it does not belong to, each observation is its own record and carries
/// a `segment` counting from one. A reader sums the segments of a phase to get
/// that phase's cost, and can still see that the wait was split.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhaseRecord {
    /// Schema version this record was written with.
    pub schema: u32,
    /// Always [`RecordKind::Phase`].
    pub kind: RecordKind,
    /// Campaign this phase belongs to.
    pub campaign_id: String,
    /// Workload this phase belongs to.
    pub workload_id: String,
    /// Repetition this phase belongs to.
    pub repetition: u32,
    /// Which phase was measured.
    pub phase: PhaseKind,
    /// Which observation of this phase within the attempt, counting from one.
    pub segment: u32,
    /// How long the phase took.
    pub duration_nanos: u64,
    /// Whether the phase reached its own success condition.
    pub status: PhaseStatus,
}

/// The outcome of one phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhaseStatus {
    /// The phase reached its success condition.
    Ok,
    /// The phase ended in a failure that the attempt reported.
    Failed,
    /// The phase completed but its success could not be confirmed.
    Unverified,
}
