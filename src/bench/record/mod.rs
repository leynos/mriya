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
mod run;

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
pub use run::{
    Lifetime,
    PhaseKind,
    PhaseRecord,
    PhaseStatus,
    RetainedStorageRecord,
    RunOutcome,
    RunRecord,
    RunRole,
    ToolchainPin,
    WorkloadClass,
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

impl CampaignLimits {
    /// Returns whether any declared limit is zero.
    ///
    /// A zero limit is not a limit: it would refuse every attempt for a reason
    /// that reads like a configuration mistake rather than a budget.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::record::CampaignLimits;
    ///
    /// let zero = CampaignLimits {
    ///     max_spend_micros: 0,
    ///     max_attempts: 1,
    ///     max_attempt_nanos: 1,
    /// };
    /// assert!(zero.declares_a_zero());
    /// ```
    #[must_use]
    pub const fn declares_a_zero(&self) -> bool {
        self.max_spend_micros == 0 || self.max_attempts == 0 || self.max_attempt_nanos == 0
    }
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

/// Serializes a record as one JSON line, without a trailing newline.
///
/// # Errors
///
/// Returns [`serde_json::Error`] when the record cannot be represented, which
/// would indicate a type whose `Serialize` implementation disagrees with its
/// fields rather than a data problem.
///
/// # Parameters
///
/// - `record`: the record to write.
///
/// # Returns
///
/// The JSON object as one line, without a trailing newline.
///
/// # Errors
///
/// Returns [`LineError::TagMismatch`] when the record's inner `kind` field
/// disagrees with the variant holding it, and [`LineError::Malformed`] when the
/// record cannot be represented.
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
/// # Ok::<(), mriya::bench::record::LineError>(())
/// ```
pub fn to_line(record: &Record) -> Result<String, LineError> {
    let expected = record.kind();
    let inner = match record {
        Record::Header(inner) => inner.kind,
        Record::Run(inner) => inner.kind,
        Record::Phase(inner) => inner.kind,
        Record::Residual(inner) => inner.kind,
    };
    if inner != expected {
        return Err(LineError::TagMismatch {
            variant: expected,
            tag: inner,
        });
    }
    serialize(record)
}

/// Serializes a record whose tag has already been checked.
fn serialize(record: &Record) -> Result<String, LineError> {
    let serialized = match record {
        Record::Header(inner) => serde_json::to_string(inner),
        Record::Run(inner) => serde_json::to_string(inner),
        Record::Phase(inner) => serde_json::to_string(inner),
        Record::Residual(inner) => serde_json::to_string(inner),
    };
    serialized.map_err(|err| LineError::Malformed(err.to_string()))
}

/// Reads one line into a tagged [`Record`].
///
/// # Parameters
///
/// - `line`: one JSON object, as written by [`to_line`].
///
/// # Returns
///
/// The decoded record. Decoding does not validate the campaign and does not
/// enforce the schema version: a successfully decoded record still has to pass
/// [`crate::bench::validate::validate_str`].
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
    // A present tag that is not a string is a malformed record, not a missing
    // one: telling an operator to add a field that is already there misleads.
    let kind_value = value.get("kind").ok_or(LineError::MissingKind)?;
    let kind_text = kind_value.as_str().ok_or_else(|| {
        LineError::Malformed(format!("kind must be a string, found {kind_value}"))
    })?;
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
        /// The unrecognized tag.
        kind: String,
    },
    /// A record's inner tag disagrees with the variant holding it.
    TagMismatch {
        /// The tag the variant implies.
        variant: RecordKind,
        /// The tag the record's own field carries.
        tag: RecordKind,
    },
}

impl fmt::Display for LineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(message) => write!(formatter, "malformed record: {message}"),
            Self::MissingKind => formatter.write_str("record has no kind field"),
            Self::UnknownKind { kind } => write!(formatter, "unknown record kind: {kind}"),
            Self::TagMismatch { variant, tag } => write!(
                formatter,
                "record is a {variant} but its kind field says {tag}"
            ),
        }
    }
}

impl std::error::Error for LineError {}
