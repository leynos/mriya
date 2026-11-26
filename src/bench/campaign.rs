//! The paid campaign harness, gated behind an explicit operator opt-in.
//!
//! Paid execution is the one thing in this repository that spends money, so the
//! harness is built around three refusals rather than three features:
//!
//! 1. It refuses to start unless the operator has set [`OPT_IN_ENV`](super::record::OPT_IN_ENV) to
//!    [`OPT_IN_VALUE`](super::record::OPT_IN_VALUE) and declared every limit.
//! 2. It refuses to allocate until a pre-sweep has found nothing left behind by an earlier
//!    campaign.
//! 3. On any cleanup doubt it appends a residual record naming every identifier it could not verify
//!    gone, and stops. It never continues to the next attempt in the hope that the problem was
//!    transient.
//!
//! Every collaborator is injected, so the policy above is testable without a
//! provider, a credential, or a spend. The production implementations live in
//! the `mriya-bench` binary, not here.

use std::fmt;

use super::record::{
    CampaignHeader,
    CampaignLimits,
    OPT_IN_ENV,
    OPT_IN_VALUE,
    Record,
    RecordKind,
    ResidualRecord,
    RunRecord,
    SCHEMA_VERSION,
    to_line,
};

/// One attempt a campaign intends to make.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedAttempt {
    /// Workload identifier from the protocol's workload set.
    pub workload_id: String,
    /// Provider the attempt targets.
    pub provider: String,
    /// Product the attempt requests.
    pub product: String,
    /// Longest the attempt is allowed to live, in nanoseconds.
    pub planned_lifetime_nanos: u64,
    /// What the attempt is expected to cost, in micro-units of the sheet's
    /// currency.
    ///
    /// The harness enforces the stated budget against this declared figure
    /// before each allocation, because an actual figure does not exist until
    /// after the money is spent.
    pub estimated_micros: u64,
}

/// Everything a campaign must declare before it may allocate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CampaignPlan {
    /// Identifier every record carries.
    pub campaign_id: String,
    /// Operator-declared limits.
    pub limits: CampaignLimits,
    /// Price sheet the campaign expects to be priced against.
    pub price_sheet: Option<String>,
    /// Providers the campaign compares.
    pub providers: Vec<String>,
    /// Attempts, in the order they run.
    pub attempts: Vec<PlannedAttempt>,
}

impl CampaignPlan {
    /// Checks that the plan is internally consistent.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::InvalidPlan`] when the campaign has no attempts,
    /// a zero limit, an attempt longer than the declared maximum lifetime, or a
    /// total estimate beyond the declared budget. Each of those would otherwise
    /// be discovered only after money had been spent.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::{
    ///     campaign::{CampaignPlan, PlannedAttempt},
    ///     record::CampaignLimits,
    /// };
    ///
    /// let plan = CampaignPlan {
    ///     campaign_id: String::from("c"),
    ///     limits: CampaignLimits {
    ///         max_spend_micros: 10,
    ///         max_attempts: 1,
    ///         max_attempt_nanos: 10,
    ///     },
    ///     price_sheet: None,
    ///     providers: vec![String::from("scaleway")],
    ///     attempts: vec![PlannedAttempt {
    ///         workload_id: String::from("cold-build"),
    ///         provider: String::from("scaleway"),
    ///         product: String::from("DEV1-S"),
    ///         planned_lifetime_nanos: 5,
    ///         estimated_micros: 5,
    ///     }],
    /// };
    /// assert!(plan.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), CampaignError> {
        if self.campaign_id.trim().is_empty() {
            return Err(CampaignError::InvalidPlan(String::from(
                "the campaign needs an identifier",
            )));
        }
        if self.attempts.is_empty() {
            return Err(CampaignError::InvalidPlan(String::from(
                "the campaign needs at least one attempt",
            )));
        }
        if self.limits.max_spend_micros == 0
            || self.limits.max_attempts == 0
            || self.limits.max_attempt_nanos == 0
        {
            return Err(CampaignError::InvalidPlan(String::from(
                "every limit must be greater than zero",
            )));
        }
        if self.attempts.len() > usize::try_from(self.limits.max_attempts).unwrap_or(0) {
            return Err(CampaignError::InvalidPlan(format!(
                "the plan lists {} attempts but the limit is {}",
                self.attempts.len(),
                self.limits.max_attempts
            )));
        }
        let mut estimated = 0_u64;
        for attempt in &self.attempts {
            if attempt.planned_lifetime_nanos > self.limits.max_attempt_nanos {
                return Err(CampaignError::InvalidPlan(format!(
                    "attempt for {} plans to live longer than the declared maximum",
                    attempt.workload_id
                )));
            }
            estimated = estimated.saturating_add(attempt.estimated_micros);
        }
        if estimated > self.limits.max_spend_micros {
            return Err(CampaignError::InvalidPlan(format!(
                "the plan estimates {estimated} micro-units but the budget is {}",
                self.limits.max_spend_micros
            )));
        }
        Ok(())
    }

    /// Builds the header record a campaign file begins with.
    #[must_use]
    pub fn header(&self) -> CampaignHeader {
        CampaignHeader {
            schema: SCHEMA_VERSION,
            kind: RecordKind::CampaignHeader,
            campaign_id: self.campaign_id.clone(),
            limits: Some(self.limits),
            price_sheet: self.price_sheet.clone(),
            providers: self.providers.clone(),
        }
    }
}

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
    pub fn verified() -> Self {
        Self {
            residual_ids: Vec::new(),
            reason: String::new(),
        }
    }

    /// Returns whether this teardown is verified.
    #[must_use]
    pub const fn is_verified(&self) -> bool { self.residual_ids.is_empty() }
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
    fn run_attempt(&self, campaign_id: &str, attempt: &PlannedAttempt)
    -> Result<RunRecord, String>;

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
}

mod scaleway_driver;

pub use scaleway_driver::ScalewayDriver;

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

/// Runs a campaign, stopping at the first doubt.
///
/// The header is written first, then the pre-sweep runs, then each attempt is
/// recorded before its teardown is verified. That order means a campaign that
/// is interrupted has already recorded everything it did.
///
/// # Errors
///
/// Returns [`CampaignError`] when the operator has not opted in, the plan is
/// invalid, the pre-sweep fails or finds residual resources, an attempt cannot
/// be recorded, a record cannot be written, or a teardown cannot be verified.
/// On a cleanup doubt the residual record is written before the error returns.
///
/// # Examples
///
/// A campaign that is not enabled does not call the driver at all:
///
/// ```
/// use mriya::bench::{
///     campaign::{
///         CampaignDriver,
///         CampaignError,
///         CampaignPlan,
///         PlannedAttempt,
///         RecordSink,
///         SweepReport,
///         TeardownReport,
///         run_campaign,
///     },
///     record::{CampaignLimits, Record, RunRecord},
/// };
///
/// struct NeverCalled;
///
/// impl CampaignDriver for NeverCalled {
///     fn pre_sweep(&self, _: &str) -> Result<SweepReport, String> {
///         panic!("the driver must not be called when paid runs are disabled")
///     }
///     fn run_attempt(&self, _: &str, _: &PlannedAttempt) -> Result<RunRecord, String> {
///         panic!("the driver must not be called when paid runs are disabled")
///     }
///     fn verify_teardown(
///         &self,
///         _: &str,
///         _: &PlannedAttempt,
///         _: &RunRecord,
///     ) -> Result<TeardownReport, String> {
///         panic!("the driver must not be called when paid runs are disabled")
///     }
/// }
///
/// struct Discard;
/// impl RecordSink for Discard {
///     fn append(&mut self, _: &Record) -> Result<(), String> { Ok(()) }
/// }
///
/// let plan = CampaignPlan {
///     campaign_id: String::from("c"),
///     limits: CampaignLimits {
///         max_spend_micros: 1,
///         max_attempts: 1,
///         max_attempt_nanos: 1,
///     },
///     price_sheet: None,
///     providers: vec![String::from("scaleway")],
///     attempts: vec![PlannedAttempt {
///         workload_id: String::from("cold-build"),
///         provider: String::from("scaleway"),
///         product: String::from("DEV1-S"),
///         planned_lifetime_nanos: 1,
///         estimated_micros: 1,
///     }],
/// };
/// let mut sink = Discard;
/// assert!(matches!(
///     run_campaign(&plan, None, &NeverCalled, &mut sink),
///     Err(CampaignError::NotEnabled { .. })
/// ));
/// ```
pub fn run_campaign<D, S>(
    plan: &CampaignPlan,
    opt_in: Option<&str>,
    driver: &D,
    sink: &mut S,
) -> Result<(), CampaignError>
where
    D: CampaignDriver,
    S: RecordSink,
{
    require_opt_in(opt_in)?;
    plan.validate()?;

    sink.append(&Record::Header(Box::new(plan.header())))
        .map_err(CampaignError::RecordWriteFailed)?;

    match driver.pre_sweep(&plan.campaign_id) {
        Ok(report) if report.residual_ids.is_empty() => {}
        Ok(report) => {
            record_residual(
                sink,
                &plan.campaign_id,
                &plan.providers,
                report.residual_ids.clone(),
                "the pre-sweep found resources from an earlier campaign",
            )?;
            return Err(CampaignError::SweepFoundResiduals(report.residual_ids));
        }
        Err(reason) => {
            record_residual(
                sink,
                &plan.campaign_id,
                &plan.providers,
                Vec::new(),
                &format!("the pre-sweep could not be completed: {reason}"),
            )?;
            return Err(CampaignError::SweepFailed(reason));
        }
    }

    for attempt in &plan.attempts {
        let run = driver
            .run_attempt(&plan.campaign_id, attempt)
            .map_err(CampaignError::AttemptFailed)?;
        sink.append(&Record::Run(Box::new(run.clone())))
            .map_err(CampaignError::RecordWriteFailed)?;

        match driver.verify_teardown(&plan.campaign_id, attempt, &run) {
            Ok(report) if report.is_verified() => {}
            Ok(report) => {
                record_residual(
                    sink,
                    &plan.campaign_id,
                    &plan.providers,
                    report.residual_ids.clone(),
                    &report.reason,
                )?;
                return Err(CampaignError::CleanupDoubt {
                    residual_ids: report.residual_ids,
                    reason: report.reason,
                });
            }
            Err(reason) => {
                record_residual(
                    sink,
                    &plan.campaign_id,
                    &plan.providers,
                    vec![run.provider.provider.clone()],
                    &reason,
                )?;
                return Err(CampaignError::CleanupDoubt {
                    residual_ids: vec![run.provider.provider.clone()],
                    reason,
                });
            }
        }
    }

    Ok(())
}

/// Writes the residual record that stops a campaign.
fn record_residual<S: RecordSink>(
    sink: &mut S,
    campaign_id: &str,
    providers: &[String],
    resource_ids: Vec<String>,
    reason: &str,
) -> Result<(), CampaignError> {
    let provider = providers.first().cloned().unwrap_or_default();
    let record = ResidualRecord {
        schema: SCHEMA_VERSION,
        kind: RecordKind::ResidualResource,
        campaign_id: campaign_id.to_owned(),
        provider,
        resource_ids,
        reason: reason.to_owned(),
    };
    sink.append(&Record::Residual(Box::new(record)))
        .map_err(CampaignError::RecordWriteFailed)
}
