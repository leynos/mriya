//! The ports the campaign harness drives, and the errors it reports.
//!
//! Every collaborator is a port so the harness's policy — opt-in, limits,
//! fail-closed cleanup — can be tested without a provider, a credential, or a
//! spend. The production implementations live in the binary and in
//! `scaleway_driver.rs`.

use std::fmt;

use super::plan::PlannedAttempt;
use crate::bench::record::{OPT_IN_ENV, OPT_IN_VALUE, PhaseRecord, Record, RunRecord, to_line};

/// What a pre-sweep found.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SweepReport {
    /// Identifiers an earlier campaign left behind.
    pub residual_ids: Vec<String>,
}

/// The outcome of verifying that one attempt's resources are gone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeardownReport {
    /// Identifiers that could not be verified gone.
    pub residual_ids: Vec<String>,
    /// Why verification failed, when it did.
    pub reason: String,
}

impl TeardownReport {
    /// Returns a verified teardown with nothing left behind.
    #[must_use]
    pub const fn verified() -> Self {
        Self {
            residual_ids: Vec::new(),
            reason: String::new(),
        }
    }

    /// Returns whether this teardown is verified.
    ///
    /// A driver that could not confirm the teardown must set `reason` even when
    /// it can name no identifier, so a report with no residual IDs but a reason
    /// is still a doubt.
    #[must_use]
    pub fn is_verified(&self) -> bool {
        self.residual_ids.is_empty() && self.reason.trim().is_empty()
    }
}

/// The records one attempt produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptRecords {
    /// The attempt's summary record.
    pub run: RunRecord,
    /// Every phase the attempt reported, in the order it finished.
    pub phases: Vec<PhaseRecord>,
    /// Provider identifiers the attempt created.
    ///
    /// A campaign needs them to write an actionable residual record when an
    /// attempt fails after allocating. An empty list means the attempt created
    /// nothing it knows of, which is different from "the provider was never
    /// asked".
    pub resource_ids: Vec<String>,
}

/// Why an attempt could not produce a record, and what it may have left behind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptFailure {
    /// What went wrong, in one sentence.
    pub reason: String,
    /// Provider identifiers the attempt created before it failed.
    ///
    /// Empty when the attempt is known to have created nothing, or when its
    /// teardown was verified before the failure. Non-empty identifiers are
    /// written into a residual record so an operator can act on them.
    pub resource_ids: Vec<String>,
}

impl AttemptFailure {
    /// Returns a failure that certainly left nothing behind.
    #[must_use]
    pub fn without_resources(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            resource_ids: Vec::new(),
        }
    }
}

/// Runs one attempt and reports the resources it used.
pub trait CampaignDriver {
    /// Lists and deletes anything an earlier run of this campaign left behind.
    ///
    /// # Errors
    ///
    /// Returns a message when the sweep could not be completed. A sweep that
    /// could not run is not a sweep that found nothing.
    fn pre_sweep(&self, campaign_id: &str) -> Result<SweepReport, String>;

    /// Runs one attempt and returns its record.
    ///
    /// # Errors
    ///
    /// Returns a message when the attempt could not produce a record at all.
    fn run_attempt(
        &self,
        campaign_id: &str,
        attempt: &PlannedAttempt,
    ) -> Result<AttemptRecords, AttemptFailure>;

    /// Verifies that the attempt's resources are gone.
    ///
    /// # Errors
    ///
    /// Returns a message when verification itself failed, which is a cleanup
    /// doubt rather than a clean result.
    fn verify_teardown(
        &self,
        campaign_id: &str,
        attempt: &PlannedAttempt,
        run: &RunRecord,
    ) -> Result<TeardownReport, String>;
}

/// Receives each record as it is produced, so an interrupted campaign leaves a
/// readable prefix.
pub trait RecordSink {
    /// Appends one record.
    ///
    /// # Errors
    ///
    /// Returns a message when the record could not be written. The campaign
    /// stops, because a record that cannot be written is evidence that cannot
    /// be recovered later.
    fn append(&mut self, record: &Record) -> Result<(), String>;

    /// Pushes any buffered records to their destination.
    ///
    /// The default is a no-op, for a sink that writes through immediately.
    ///
    /// # Errors
    ///
    /// Returns a message when the destination rejects the data, so a caller
    /// never reports success for records that were lost in a buffer.
    fn flush(&mut self) -> Result<(), String> { Ok(()) }
}

/// Writes records as JSON Lines to any [`std::io::Write`].
#[derive(Debug)]
pub struct JsonLinesSink<W: std::io::Write> {
    /// Destination for the records.
    writer: W,
}

impl<W: std::io::Write> JsonLinesSink<W> {
    /// Wraps `writer`.
    #[must_use]
    pub const fn new(writer: W) -> Self { Self { writer } }

    /// Returns the wrapped writer.
    pub fn into_inner(self) -> W { self.writer }
}

impl<W: std::io::Write> RecordSink for JsonLinesSink<W> {
    fn append(&mut self, record: &Record) -> Result<(), String> {
        let line = to_line(record).map_err(|err| err.to_string())?;
        writeln!(self.writer, "{line}").map_err(|err| err.to_string())
    }

    fn flush(&mut self) -> Result<(), String> { self.writer.flush().map_err(|err| err.to_string()) }
}

/// Why a campaign stopped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CampaignError {
    /// The operator has not enabled paid runs.
    NotEnabled {
        /// The variable the operator must set.
        variable: &'static str,
        /// The value it must carry.
        value: &'static str,
    },
    /// The plan itself is not usable.
    InvalidPlan(String),
    /// The pre-sweep could not be completed.
    SweepFailed(String),
    /// The pre-sweep found resources from an earlier campaign.
    SweepFoundResiduals(Vec<String>),
    /// An attempt could not produce a record.
    AttemptFailed(String),
    /// A teardown could not be verified.
    CleanupDoubt {
        /// Identifiers that could not be verified gone.
        residual_ids: Vec<String>,
        /// Why verification failed.
        reason: String,
    },
    /// A record could not be written.
    RecordWriteFailed(String),
}

impl fmt::Display for CampaignError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEnabled { variable, value } => write!(
                formatter,
                "paid benchmark runs are disabled; set {variable}={value} to enable them"
            ),
            Self::InvalidPlan(message) => write!(formatter, "campaign plan is invalid: {message}"),
            Self::SweepFailed(message) => {
                write!(formatter, "pre-sweep could not be completed: {message}")
            }
            Self::SweepFoundResiduals(ids) => {
                write!(formatter, "pre-sweep found residual resources: {ids:?}")
            }
            Self::AttemptFailed(message) => write!(formatter, "attempt failed: {message}"),
            Self::CleanupDoubt {
                residual_ids,
                reason,
            } => write!(
                formatter,
                "cleanup could not be verified, campaign stopped; residual resources {residual_ids:?}: \
                 {reason}"
            ),
            Self::RecordWriteFailed(message) => {
                write!(formatter, "a record could not be written: {message}")
            }
        }
    }
}

impl std::error::Error for CampaignError {}

/// Returns the opt-in verdict for an environment.
///
/// # Errors
///
/// Returns [`CampaignError::NotEnabled`] unless `value` is exactly
/// [`OPT_IN_VALUE`]. A value of `true`, `yes`, or `0` is not accepted: an
/// operator enabling spending should have to type the documented value.
///
/// # Examples
///
/// ```
/// use mriya::bench::campaign::require_opt_in;
///
/// assert!(require_opt_in(Some("1")).is_ok());
/// assert!(require_opt_in(None).is_err());
/// assert!(require_opt_in(Some("yes")).is_err());
/// ```
pub fn require_opt_in(value: Option<&str>) -> Result<(), CampaignError> {
    if value.map(str::trim) == Some(OPT_IN_VALUE) {
        return Ok(());
    }
    Err(CampaignError::NotEnabled {
        variable: OPT_IN_ENV,
        value: OPT_IN_VALUE,
    })
}
