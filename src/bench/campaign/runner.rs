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
/// # Parameters
///
/// - `plan`: the operator's declared campaign.
/// - `opt_in`: the value of the opt-in variable, if set.
/// - `driver`: the provider implementation.
/// - `sink`: where each record is appended and flushed.
///
/// # Returns
///
/// `Ok(())` after every attempt completed and the final record was flushed.
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
///     fn provider(&self) -> &'static str { "scaleway" }
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
    check_served_provider(plan, driver)?;
    // Held for the whole campaign, so a second invocation cannot pass its own
    // sweep while this one is allocating.
    let _lock = super::CampaignLock::acquire(&plan.campaign_id)?;

    sink.append(&Record::Header(Box::new(plan.header())))
        .map_err(|message| CampaignError::RecordWriteFailed {
            message,
            residual: None,
        })?;
    pre_sweep(plan, driver, sink)?;

    for attempt in &plan.attempts {
        run_one_attempt(plan, attempt, driver, sink)?;
    }

    sink.flush()
        .map_err(|message| CampaignError::RecordWriteFailed {
            message,
            residual: None,
        })
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
        Err(failure) => return abandon_attempt(plan, attempt, failure, sink),
    };
    // A successful attempt has already run, so its teardown is verified even
    // when its record cannot be written: the operator needs to know whether
    // anything it created survived.
    let write_failure = append_attempt(records.as_records(), sink).err();
    let teardown = driver.verify_teardown(&plan.campaign_id, attempt, &records.run);
    if let Some(message) = write_failure {
        return Err(CampaignError::RecordWriteFailed {
            message,
            residual: residual_detail(attempt, &records.resource_ids, teardown),
        });
    }
    match teardown {
        Ok(report) if report.is_verified() => Ok(()),
        Ok(report) => {
            // A report may give a reason without identifiers, because the
            // driver could not list them. The attempt's own identifiers are
            // then the only names an operator can act on, so they are used
            // rather than recording a doubt that names nothing.
            let residual_ids = if report.residual_ids.is_empty() {
                records.resource_ids.clone()
            } else {
                report.residual_ids
            };
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

/// Appends an attempt's records, returning the first failure.
fn append_attempt<S: RecordSink>(records: Vec<Record>, sink: &mut S) -> Result<(), String> {
    for record in records {
        sink.append(&record)?;
    }
    Ok(())
}

/// Describes what a write failure left behind, when anything is known.
fn residual_detail(
    attempt: &PlannedAttempt,
    resource_ids: &[String],
    teardown: Result<super::ports::TeardownReport, String>,
) -> Option<String> {
    let (residual_ids, reason) = match teardown {
        Ok(report) if report.is_verified() && resource_ids.is_empty() => return None,
        Ok(report) => (report.residual_ids, report.reason),
        Err(reason) => (resource_ids.to_vec(), reason),
    };
    let ids = if residual_ids.is_empty() {
        resource_ids.to_vec()
    } else {
        residual_ids
    };
    Some(format!(
        "provider {:?} resources {ids:?}: teardown status {reason:?}",
        attempt.provider
    ))
}

/// Records a failed attempt as a cleanup doubt and stops the campaign.
fn abandon_attempt<S: RecordSink>(
    plan: &CampaignPlan,
    attempt: &PlannedAttempt,
    failure: super::ports::AttemptFailure,
    sink: &mut S,
) -> Result<(), CampaignError> {
    record_residual(
        sink,
        plan,
        ResidualDetail {
            provider: attempt.provider.clone(),
            resource_ids: failure.resource_ids.clone(),
            reason: format!(
                "the attempt for {} did not complete, so its resources could not be verified \
                 gone: {}{}",
                attempt.workload_id,
                failure.reason,
                if failure.allocation_uncertain {
                    " (a create may have produced a resource whose identifier is unknown; list \
                     the provider's resources in the target zone)"
                } else {
                    ""
                }
            ),
        },
    )?;
    Err(CampaignError::AttemptFailed(failure.reason))
}

/// Requires every attempt to target the provider the driver serves.
///
/// A plan can declare several providers, but a campaign is run by one driver.
/// Checking before the sweep means a mismatched plan allocates nothing at all.
fn check_served_provider<D: CampaignDriver>(
    plan: &CampaignPlan,
    driver: &D,
) -> Result<(), CampaignError> {
    let served = driver.provider();
    for attempt in &plan.attempts {
        if attempt.provider != served {
            return Err(CampaignError::InvalidPlan(format!(
                "attempt for {} targets provider {:?}, but this driver serves {served:?}",
                attempt.workload_id, attempt.provider
            )));
        }
    }
    Ok(())
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
                    provider: driver.provider().to_owned(),
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
                    provider: driver.provider().to_owned(),
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
    // Captured before the detail moves into the record: if the record cannot be
    // persisted, the error must still name what the operator has to clean up.
    let named = format!(
        "provider {:?} resources {:?}: {}",
        detail.provider, detail.resource_ids, detail.reason
    );
    let record = ResidualRecord {
        schema: SCHEMA_VERSION,
        kind: RecordKind::ResidualResource,
        campaign_id: plan.campaign_id.clone(),
        provider: detail.provider,
        resource_ids: detail.resource_ids,
        reason: detail.reason,
    };
    sink.append(&Record::Residual(Box::new(record)))
        .map_err(|message| CampaignError::RecordWriteFailed {
            message,
            residual: Some(named.clone()),
        })?;
    // A residual record is the evidence an operator acts on after a failure, so
    // it is flushed before the campaign returns, not left in a buffer that a
    // dropped writer may never empty.
    sink.flush()
        .map_err(|message| CampaignError::RecordWriteFailed {
            message,
            residual: Some(named),
        })
}
