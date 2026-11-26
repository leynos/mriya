//! Running a campaign, and stopping it at the first doubt.
//!
//! The order here is the safety property: write the header, sweep, then for
//! each attempt record the run and its phases before verifying teardown. A
//! campaign that is interrupted has therefore already recorded everything it
//! did, and a cleanup doubt leaves a residual record naming what remains.

use super::{
    CampaignPlan,
    PlannedAttempt,
    ports::{CampaignDriver, CampaignError, RecordSink, require_opt_in},
};
use crate::bench::record::{Record, RecordKind, ResidualRecord, SCHEMA_VERSION};

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
///         AttemptFailure,
///         AttemptRecords,
///         CampaignDriver,
///         CampaignError,
///         CampaignPlan,
///         PlannedAttempt,
///         RecordSink,
///         SweepReport,
///         TeardownReport,
///         run_campaign,
///     },
///     record::{CampaignLimits, Record, RunRecord, RunRole},
/// };
///
/// struct NeverCalled;
///
/// impl CampaignDriver for NeverCalled {
///     fn pre_sweep(&self, _: &str) -> Result<SweepReport, String> {
///         panic!("the driver must not be called when paid runs are disabled")
///     }
///     fn run_attempt(
///         &self,
///         _: &str,
///         _: &PlannedAttempt,
///     ) -> Result<AttemptRecords, AttemptFailure> {
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
///         repetition: 1,
///         role: RunRole::Timed,
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
    pre_sweep(plan, driver, sink)?;

    for attempt in &plan.attempts {
        run_one_attempt(plan, attempt, driver, sink)?;
    }

    sink.flush().map_err(CampaignError::RecordWriteFailed)
}

/// Runs one attempt, records it, and verifies its teardown.
///
/// The records are written before the teardown check, so an attempt that
/// cannot be verified still leaves its measurements behind.
fn run_one_attempt<D: CampaignDriver, S: RecordSink>(
    plan: &CampaignPlan,
    attempt: &PlannedAttempt,
    driver: &D,
    sink: &mut S,
) -> Result<(), CampaignError> {
    let records = match driver.run_attempt(&plan.campaign_id, attempt) {
        Ok(records) => records,
        Err(failure) => {
            // An attempt that failed may have allocated before it failed. The
            // identifiers it reported are recorded, and the campaign stops:
            // continuing would allocate on top of an unknown residue.
            record_residual(
                sink,
                plan,
                ResidualDetail {
                    provider: attempt.provider.clone(),
                    resource_ids: failure.resource_ids.clone(),
                    reason: format!(
                        "the attempt for {} did not complete, so its resources could not be \
                         verified gone: {}",
                        attempt.workload_id, failure.reason
                    ),
                },
            )?;
            return Err(CampaignError::AttemptFailed(failure.reason));
        }
    };
    sink.append(&Record::Run(Box::new(records.run.clone())))
        .map_err(CampaignError::RecordWriteFailed)?;
    for phase in &records.phases {
        sink.append(&Record::Phase(Box::new(phase.clone())))
            .map_err(CampaignError::RecordWriteFailed)?;
    }
    match driver.verify_teardown(&plan.campaign_id, attempt, &records.run) {
        Ok(report) if report.is_verified() => Ok(()),
        Ok(report) => {
            let residual_ids = report.residual_ids;
            record_residual(
                sink,
                plan,
                ResidualDetail {
                    provider: attempt.provider.clone(),
                    resource_ids: residual_ids.clone(),
                    reason: report.reason.clone(),
                },
            )?;
            Err(CampaignError::CleanupDoubt {
                residual_ids,
                reason: report.reason,
            })
        }
        Err(reason) => {
            // The driver could not confirm teardown. Its own identifiers are
            // the only ones a reader can act on, so they are recorded; an
            // empty list is still a doubt because the reason says so.
            let residual_ids = records.resource_ids.clone();
            record_residual(
                sink,
                plan,
                ResidualDetail {
                    provider: attempt.provider.clone(),
                    resource_ids: residual_ids.clone(),
                    reason: reason.clone(),
                },
            )?;
            Err(CampaignError::CleanupDoubt {
                residual_ids,
                reason,
            })
        }
    }
}

/// Runs the pre-sweep, stopping the campaign when it finds anything.
///
/// A sweep that could not run is treated exactly like a sweep that found
/// something: the campaign has no evidence that the account is clean, and
/// allocating on top of an unknown residue is how a leak becomes permanent.
fn pre_sweep<D: CampaignDriver, S: RecordSink>(
    plan: &CampaignPlan,
    driver: &D,
    sink: &mut S,
) -> Result<(), CampaignError> {
    match driver.pre_sweep(&plan.campaign_id) {
        Ok(report) if report.residual_ids.is_empty() => Ok(()),
        Ok(report) => {
            record_residual(
                sink,
                plan,
                ResidualDetail {
                    provider: plan.providers.first().cloned().unwrap_or_default(),
                    resource_ids: report.residual_ids.clone(),
                    reason: String::from("the pre-sweep found resources from an earlier campaign"),
                },
            )?;
            Err(CampaignError::SweepFoundResiduals(report.residual_ids))
        }
        Err(reason) => {
            record_residual(
                sink,
                plan,
                ResidualDetail {
                    provider: plan.providers.first().cloned().unwrap_or_default(),
                    resource_ids: Vec::new(),
                    reason: format!("the pre-sweep could not be completed: {reason}"),
                },
            )?;
            Err(CampaignError::SweepFailed(reason))
        }
    }
}

/// What a residual record says beyond the campaign it belongs to.
struct ResidualDetail {
    /// Provider the failing attempt targeted.
    provider: String,
    /// Identifiers that could not be verified gone.
    resource_ids: Vec<String>,
    /// Why the campaign could not confirm teardown.
    reason: String,
}

/// Writes the residual record that stops a campaign.
fn record_residual<S: RecordSink>(
    sink: &mut S,
    plan: &CampaignPlan,
    detail: ResidualDetail,
) -> Result<(), CampaignError> {
    let record = ResidualRecord {
        schema: SCHEMA_VERSION,
        kind: RecordKind::ResidualResource,
        campaign_id: plan.campaign_id.clone(),
        provider: detail.provider,
        resource_ids: detail.resource_ids,
        reason: detail.reason,
    };
    sink.append(&Record::Residual(Box::new(record)))
        .map_err(CampaignError::RecordWriteFailed)
}
