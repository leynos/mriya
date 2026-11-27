//! Residual records: how a campaign names what it could not verify gone.
//!
//! A campaign stops at the first cleanup doubt. The record it leaves behind is
//! the only thing an operator has to act on, so every identifier the harness
//! knows is written into it — including the attempt's own identifiers when the
//! driver could not list them. A failure that names nothing and reports no
//! uncertainty is not a cleanup doubt at all: it writes no residual.

use super::{
    CampaignPlan,
    PlannedAttempt,
    ports::{CampaignError, RecordSink, TeardownReport},
};
use crate::bench::record::{Record, RecordKind, ResidualRecord, SCHEMA_VERSION};

/// What a residual record says beyond the campaign it belongs to.
pub(super) struct ResidualDetail {
    /// Provider the failing attempt targeted.
    pub(super) provider: String,
    /// Identifiers that could not be verified gone.
    pub(super) resource_ids: Vec<String>,
    /// Why the campaign could not confirm teardown.
    pub(super) reason: String,
}

/// Records a failed attempt as a cleanup doubt and stops the campaign.
pub(super) fn abandon_attempt<S: RecordSink>(
    plan: &CampaignPlan,
    attempt: &PlannedAttempt,
    failure: super::ports::AttemptFailure,
    sink: &mut S,
) -> Result<(), CampaignError> {
    let reason = failure.reason;
    // A failure that names no resource and reports no uncertainty states that
    // nothing remains — a probe that could not be read after a verified
    // teardown, for example. Recording a cleanup doubt for it would tell an
    // operator to look for resources that do not exist.
    if failure.resource_ids.is_empty() && !failure.allocation_uncertain {
        return Err(CampaignError::AttemptFailed(reason));
    }
    record_residual(
        sink,
        plan,
        ResidualDetail {
            provider: attempt.provider.clone(),
            resource_ids: failure.resource_ids.clone(),
            reason: format!(
                "the attempt for {} did not complete, so its resources could not be verified \
                 gone: {reason}{}",
                attempt.workload_id,
                if failure.allocation_uncertain {
                    " (a create may have produced a resource whose identifier is unknown; list \
                     the provider's resources in the target zone)"
                } else {
                    ""
                }
            ),
        },
    )?;
    Err(CampaignError::AttemptFailed(reason))
}

/// Describes what a write failure left behind, when anything is known.
pub(super) fn residual_detail(
    attempt: &PlannedAttempt,
    resource_ids: &[String],
    teardown: Result<TeardownReport, String>,
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

/// Writes the residual record that stops a campaign.
pub(super) fn record_residual<S: RecordSink>(
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
