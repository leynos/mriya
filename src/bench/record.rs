//! Machine-readable benchmark records for the backend cost protocol.
//!
//! A campaign file is JSON Lines: one object per line, tagged by [`RecordKind`]
//! and versioned by [`SCHEMA_VERSION`]. Every quantity in a record is an
//! integer — durations in nanoseconds, sizes in bytes, money in micro-units of
//! the price sheet's currency — because the workspace denies float arithmetic
//! and an integer record cannot drift between two readers.
//!
//! The types here are deliberately closed: a record can only be built from
//! values that were measured or declared, so a credential has no field to hide
//! in. [`crate::bench::validate`] then checks the pins the protocol requires.

use std::fmt;

use serde::{Deserialize, Serialize};

mod pins;

pub use pins::{
    Allocation,
    CacheObservation,
    CacheState,
    Concurrency,
    GpuSpec,
    GuestArchitecture,
    ProviderProduct,
    TreePin,
};

/// Schema version written into and expected from every record.
pub const SCHEMA_VERSION: u32 = 1;

/// Environment variable an operator sets to enable paid benchmark runs.
pub const OPT_IN_ENV: &str = "MRIYA_BENCH_ENABLE_PAID_RUNS";

/// The value [`OPT_IN_ENV`] must carry for a paid run to proceed.
pub const OPT_IN_VALUE: &str = "1";

/// Tag that decides how a line is read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordKind {
    /// The first line of a campaign file: what is being measured and where.
    CampaignHeader,
    /// One attempt at one workload on one backend.
    Run,
    /// One phase of one attempt, recorded separately from the run summary.
    Phase,
    /// A resource that could not be verified gone; ends the campaign.
    ResidualResource,
}

impl RecordKind {
    /// Returns the tag as written to the file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CampaignHeader => "campaign-header",
            Self::Run => "run",
            Self::Phase => "phase",
            Self::ResidualResource => "residual-resource",
        }
    }
}

impl fmt::Display for RecordKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One attempt at one workload on one backend.
///
/// The record holds the pins, the measured lifetime, and the outcome. It holds
/// no phase timings: those are separate [`PhaseRecord`] lines so a partially
/// written file still yields the phases that completed.
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
    pub allocation: Allocation,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunRole {
    /// A measured attempt, included in the timing summary.
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
    /// Time the provider will actually bill, after granularity and minimum.
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

/// The first line of a campaign file: what is measured, and by what rules.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CampaignHeader {
    /// Schema version this record was written with.
    pub schema: u32,
    /// Always [`RecordKind::CampaignHeader`].
    pub kind: RecordKind,
    /// Identifier every record in this campaign carries.
    pub campaign_id: String,
    /// Operator-declared limits, absent on an offline replay.
    pub limits: Option<CampaignLimits>,
    /// Price-sheet identifier the campaign is expected to be priced against.
    pub price_sheet: Option<String>,
    /// Provider names the campaign intends to compare.
    pub providers: Vec<String>,
}

/// The limits an operator declares before any paid run may start.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CampaignLimits {
    /// Most money the campaign may spend, in micro-units.
    pub max_spend_micros: u64,
    /// Most attempts the campaign may make.
    pub max_attempts: u32,
    /// Longest any one attempt may live, in nanoseconds.
    pub max_attempt_nanos: u64,
}

/// A resource that could not be verified gone.
///
/// Writing one of these stops the campaign. It names every identifier the
/// janitor could not confirm deleted, so an operator can clean up by hand.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResidualRecord {
    /// Schema version this record was written with.
    pub schema: u32,
    /// Always [`RecordKind::ResidualResource`].
    pub kind: RecordKind,
    /// Campaign that stopped.
    pub campaign_id: String,
    /// Provider that still holds the resources.
    pub provider: String,
    /// Identifiers that could not be verified gone.
    pub resource_ids: Vec<String>,
    /// Why the campaign could not confirm teardown.
    pub reason: String,
}

/// Any record read from a campaign file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Record {
    /// A campaign header.
    Header(Box<CampaignHeader>),
    /// One attempt.
    Run(Box<RunRecord>),
    /// One phase of one attempt.
    Phase(Box<PhaseRecord>),
    /// A resource that could not be verified gone.
    Residual(Box<ResidualRecord>),
}

impl Record {
    /// Returns the record's kind tag.
    #[must_use]
    pub const fn kind(&self) -> RecordKind {
        match self {
            Self::Header(_) => RecordKind::CampaignHeader,
            Self::Run(_) => RecordKind::Run,
            Self::Phase(_) => RecordKind::Phase,
            Self::Residual(_) => RecordKind::ResidualResource,
        }
    }

    /// Returns the schema version the record declares.
    #[must_use]
    pub const fn schema(&self) -> u32 {
        match self {
            Self::Header(record) => record.schema,
            Self::Run(record) => record.schema,
            Self::Phase(record) => record.schema,
            Self::Residual(record) => record.schema,
        }
    }
}

/// Serialises a record as one JSON line, without a trailing newline.
///
/// # Errors
///
/// Returns [`serde_json::Error`] when the record cannot be represented, which
/// would indicate a type whose `Serialize` implementation disagrees with its
/// fields rather than a data problem.
///
/// # Examples
///
/// ```
/// use mriya::bench::record::{CampaignHeader, Record, RecordKind, SCHEMA_VERSION, to_line};
///
/// let header = CampaignHeader {
///     schema: SCHEMA_VERSION,
///     kind: RecordKind::CampaignHeader,
///     campaign_id: String::from("c1"),
///     limits: None,
///     price_sheet: None,
///     providers: vec![String::from("scaleway")],
/// };
/// let line = to_line(&Record::Header(Box::new(header)))?;
/// assert!(line.starts_with(r#"{"schema":1,"kind":"campaign-header""#));
/// # Ok::<(), serde_json::Error>(())
/// ```
pub fn to_line(record: &Record) -> Result<String, serde_json::Error> {
    match record {
        Record::Header(inner) => serde_json::to_string(inner),
        Record::Run(inner) => serde_json::to_string(inner),
        Record::Phase(inner) => serde_json::to_string(inner),
        Record::Residual(inner) => serde_json::to_string(inner),
    }
}

/// Reads one line into a tagged [`Record`].
///
/// # Errors
///
/// Returns [`LineError`] when the line is not an object, names an unknown kind,
/// or does not match the shape that kind requires.
///
/// # Examples
///
/// ```
/// use mriya::bench::record::{LineError, Record, parse_line};
///
/// assert!(matches!(
///     parse_line("not json"),
///     Err(LineError::Malformed(_))
/// ));
/// assert!(matches!(
///     parse_line(r#"{"schema":1,"kind":"unknown"}"#),
///     Err(LineError::UnknownKind { .. })
/// ));
/// ```
pub fn parse_line(line: &str) -> Result<Record, LineError> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|err| LineError::Malformed(err.to_string()))?;
    let kind_text = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or(LineError::MissingKind)?;
    let kind: RecordKind = serde_json::from_value(serde_json::Value::String(kind_text.to_owned()))
        .map_err(|_| LineError::UnknownKind {
            kind: kind_text.to_owned(),
        })?;
    let malformed = |err: serde_json::Error| LineError::Malformed(err.to_string());
    match kind {
        RecordKind::CampaignHeader => serde_json::from_value(value)
            .map(|record| Record::Header(Box::new(record)))
            .map_err(malformed),
        RecordKind::Run => serde_json::from_value(value)
            .map(|record| Record::Run(Box::new(record)))
            .map_err(malformed),
        RecordKind::Phase => serde_json::from_value(value)
            .map(|record| Record::Phase(Box::new(record)))
            .map_err(malformed),
        RecordKind::ResidualResource => serde_json::from_value(value)
            .map(|record| Record::Residual(Box::new(record)))
            .map_err(malformed),
    }
}

/// Why a line could not be read as a record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LineError {
    /// The line is not valid JSON, or does not match the shape its kind needs.
    Malformed(String),
    /// The line has no `kind` field.
    MissingKind,
    /// The `kind` field names a tag this version does not define.
    UnknownKind {
        /// The unrecognised tag.
        kind: String,
    },
}

impl fmt::Display for LineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(message) => write!(formatter, "malformed record: {message}"),
            Self::MissingKind => formatter.write_str("record has no kind field"),
            Self::UnknownKind { kind } => write!(formatter, "unknown record kind: {kind}"),
        }
    }
}

impl std::error::Error for LineError {}
