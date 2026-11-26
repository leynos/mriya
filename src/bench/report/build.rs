//! Building a priced report from a validated campaign.
//!
//! Attempts are priced in the order the file records them, so a plan's
//! included compute is consumed by the earliest attempts. That ordering is why
//! a campaign file is append-only and why the builder takes the file's order
//! rather than re-sorting by duration.

use std::collections::{BTreeMap, BTreeSet};

use super::{BenchmarkReport, ReportError, WorkloadResult};
use crate::bench::{
    billing::{CostBreakdown, PriceSheet, Totals, Usage, charge},
    record::{CacheState, PhaseKind, RunOutcome, RunRecord, RunRole},
    validate::Campaign,
};

/// Tracks how much of each provider plan's included compute remains.
struct PlanAllowances {
    /// Remaining included nanoseconds per provider.
    remaining: BTreeMap<String, u64>,
    /// Starts already used per provider.
    starts_used: BTreeMap<String, u64>,
}

impl PlanAllowances {
    /// Reads each provider's plan from the sheet.
    fn new(sheet: &PriceSheet, runs: &[(usize, RunRecord)]) -> Self {
        let mut remaining = BTreeMap::new();
        for (_, run) in runs {
            if let Some(provider) = sheet.providers.get(&run.provider.provider)
                && let Some(plan) = provider.plan.as_ref()
            {
                remaining
                    .entry(run.provider.provider.clone())
                    .or_insert(plan.included_compute_nanos);
            }
        }
        Self {
            remaining,
            starts_used: BTreeMap::new(),
        }
    }

    /// Returns the allowance left for `provider`.
    fn allowance(&self, provider: &str) -> u64 {
        self.remaining.get(provider).copied().unwrap_or(0)
    }

    /// Returns how many starts had been used before this gate.
    fn starts_used(&self, provider: &str) -> u64 {
        self.starts_used.get(provider).copied().unwrap_or(0)
    }

    /// Consumes `nanos` of allowance and one start.
    fn consume(&mut self, provider: &str, nanos: u64) {
        if let Some(remaining) = self.remaining.get_mut(provider) {
            *remaining = remaining.saturating_sub(nanos);
        }
        let starts = self.starts_used.entry(provider.to_owned()).or_insert(0);
        *starts = starts.saturating_add(1);
    }
}

/// Prices one run, consuming plan allowance as it goes.
fn price_run(
    run: &RunRecord,
    sheet: &PriceSheet,
    allowance: &mut PlanAllowances,
) -> Result<CostBreakdown, ReportError> {
    if run.lifetime.chargeable_nanos < run.lifetime.total_nanos {
        return Err(ReportError::Incoherent {
            workload_id: run.workload_id.clone(),
            message: String::from(
                "chargeable lifetime is shorter than the measured lifetime, so the record cannot \
                 have been priced from its own measurement",
            ),
        });
    }
    // The subscription is attributed once, at campaign level, so a per-run
    // figure does not multiply the plan price by the number of attempts. The
    // campaign total adds it back exactly once.
    // The record's chargeable lifetime is what the writer measured the
    // provider to bill, and it is never shorter than the attempt. Pricing from
    // it (and letting the sheet round again, which is idempotent) keeps a
    // record that already applied a minimum from being re-rounded downwards.
    let usage = Usage::new(
        run.provider.provider.clone(),
        run.provider.product.clone(),
        run.lifetime.chargeable_nanos,
    )
    .with_plan_allowance(allowance.allowance(&run.provider.provider))
    .with_starts_used(allowance.starts_used(&run.provider.provider));
    let breakdown = charge(sheet, &usage).map_err(|err| ReportError::Billing(err.to_string()))?;
    allowance.consume(&run.provider.provider, breakdown.absorbed_nanos);
    Ok(breakdown)
}

/// Accumulates one provider-product group while the report is built.
struct GroupBuilder {
    /// Provider name.
    provider: String,
    /// Product name.
    product: String,
    /// Workload identifier.
    workload_id: String,
    /// Cache class the first attempt declared.
    cache_state: CacheState,
    /// Every cache class the group's attempts declared.
    cache_states: BTreeSet<CacheState>,
    /// Cache bytes the first attempt declared.
    cache_bytes: u64,
    /// Completed attempts.
    completed: u32,
    /// Failed attempts.
    failed: u32,
    /// Execution durations of completed attempts.
    executions_nanos: Vec<u64>,
    /// Marginal cost of completed attempts.
    completed_marginal_micros: u64,
    /// Marginal cost of failed attempts.
    failed_marginal_micros: u64,
    /// Marginal cost of warm-up attempts.
    warm_up_marginal_micros: u64,
    /// Subscription share attributed to completed attempts.
    subscription_micros: u64,
    /// Total-account cost of completed attempts.
    total_account_micros: u64,
    /// Absorbed compute time.
    absorbed_nanos: u64,
    /// Chargeable lifetime.
    chargeable_nanos: u64,
}

impl GroupBuilder {
    /// Starts a group from the first attempt that lands in it.
    fn new(run: &RunRecord) -> Self {
        Self {
            provider: run.provider.provider.clone(),
            product: run.provider.product.clone(),
            workload_id: run.workload_id.clone(),
            cache_state: run.cache.state,
            cache_states: BTreeSet::from([run.cache.state]),
            cache_bytes: run.cache.size_bytes,
            completed: 0,
            failed: 0,
            executions_nanos: Vec::new(),
            completed_marginal_micros: 0,
            failed_marginal_micros: 0,
            warm_up_marginal_micros: 0,
            subscription_micros: 0,
            total_account_micros: 0,
            absorbed_nanos: 0,
            chargeable_nanos: 0,
        }
    }

    /// Adds one priced attempt to the group.
    fn push(&mut self, run: &RunRecord, breakdown: &CostBreakdown, remote_execution: Option<&u64>) {
        self.cache_states.insert(run.cache.state);
        self.absorbed_nanos = self.absorbed_nanos.saturating_add(breakdown.absorbed_nanos);
        self.chargeable_nanos = self
            .chargeable_nanos
            .saturating_add(breakdown.chargeable_nanos);
        // A warm-up exists only to populate a cache: the protocol records its
        // cost but excludes it from the timing summary and from the per-gate
        // figure, which is about completed timed work.
        if run.role == RunRole::WarmUp {
            self.warm_up_marginal_micros = self
                .warm_up_marginal_micros
                .saturating_add(breakdown.marginal_micros);
            return;
        }
        match run.outcome {
            RunOutcome::Completed { .. } => {
                self.completed = self.completed.saturating_add(1);
                // The summary is about the workload, so it uses the
                // remote-execution phase when one was recorded and falls back
                // to the whole attempt only when it was not.
                self.executions_nanos.push(
                    remote_execution
                        .copied()
                        .unwrap_or(run.lifetime.total_nanos),
                );
                self.completed_marginal_micros = self
                    .completed_marginal_micros
                    .saturating_add(breakdown.marginal_micros);
                self.subscription_micros = self
                    .subscription_micros
                    .saturating_add(breakdown.subscription_micros);
                self.total_account_micros = self
                    .total_account_micros
                    .saturating_add(breakdown.total_account_micros);
            }
            RunOutcome::Failed { .. } | RunOutcome::Aborted { .. } => {
                self.failed = self.failed.saturating_add(1);
                self.failed_marginal_micros = self
                    .failed_marginal_micros
                    .saturating_add(breakdown.marginal_micros);
            }
        }
    }

    /// Finishes the group.
    fn finish(self) -> WorkloadResult {
        WorkloadResult {
            provider: self.provider,
            product: self.product,
            workload_id: self.workload_id,
            cache_state: self.cache_state,
            cache_states: self.cache_states,
            cache_bytes: self.cache_bytes,
            completed: self.completed,
            failed: self.failed,
            executions_nanos: self.executions_nanos,
            completed_marginal_micros: self.completed_marginal_micros,
            failed_marginal_micros: self.failed_marginal_micros,
            warm_up_marginal_micros: self.warm_up_marginal_micros,
            subscription_micros: self.subscription_micros,
            total_account_micros: self.total_account_micros,
            absorbed_nanos: self.absorbed_nanos,
            chargeable_nanos: self.chargeable_nanos,
        }
    }
}

/// Builds the report for a validated campaign.
///
/// Attempts are priced in the order the file records them, so a plan's included
/// compute is consumed by the earliest attempts. That ordering is why a campaign
/// file is append-only and why the report takes the file's order rather than
/// re-sorting by duration.
///
/// # Errors
///
/// Returns [`ReportError`] when a run cannot be priced against the sheet, or
/// when its recorded lifetime contradicts its own measurement.
///
/// # Examples
///
/// A campaign whose runs name a provider the sheet does not describe is
/// refused rather than priced at zero:
///
/// ```
/// use mriya::bench::{report::build_report, validate::validate_str};
///
/// let campaign = validate_str(concat!(
///     r#"{"schema":1,"kind":"campaign-header","campaign_id":"c","limits":null,"#,
///     r#""price_sheet":null,"providers":["absent"]}"#,
///     "\n",
/// ))
/// .unwrap_err();
/// assert!(!campaign.is_empty());
/// ```
pub fn build_report(
    campaign: &Campaign,
    sheet: &PriceSheet,
) -> Result<BenchmarkReport, ReportError> {
    let mut allowance = PlanAllowances::new(sheet, &campaign.runs);
    let mut totals = Totals::default();
    let mut grouped: BTreeMap<String, BTreeMap<(String, String), GroupBuilder>> = BTreeMap::new();

    let execution_phases = remote_execution_phases(campaign);
    for (_, run) in &campaign.runs {
        let breakdown = price_run(run, sheet, &mut allowance)?;
        breakdown
            .accumulate(&mut totals)
            .map_err(|err| ReportError::Billing(format!("campaign totals overflowed: {err}")))?;
        grouped
            .entry(run.workload_id.clone())
            .or_default()
            .entry((run.provider.provider.clone(), run.provider.product.clone()))
            .or_insert_with(|| GroupBuilder::new(run))
            .push(
                run,
                &breakdown,
                execution_phases.get(&(run.workload_id.clone(), run.repetition)),
            );
    }

    // A plan is charged once for the campaign, not once per attempt. Adding it
    // after the per-run loop is what keeps the two figures distinct: the
    // marginal totals say what the gates cost, and the account total says what
    // the campaign adds to the bill.
    let subscription_micros =
        super::campaign_subscription_micros(campaign, sheet).map_err(ReportError::Billing)?;
    totals.subscription_micros = subscription_micros;
    totals.total_account_micros = totals
        .total_account_micros
        .saturating_add(subscription_micros);

    let mut workloads = BTreeMap::new();
    let mut capability_notes = Vec::new();
    let mut comparability_problems = Vec::new();
    for (workload_id, groups) in grouped {
        let results: Vec<WorkloadResult> = groups
            .into_values()
            .map(GroupBuilder::finish)
            .collect::<Vec<_>>();
        let attributed = attribute_subscription(results, subscription_micros);
        let mut notes = super::rules::DifferenceNotes {
            capability: &mut capability_notes,
            comparability: &mut comparability_problems,
        };
        super::rules::note_differences(&workload_id, &attributed, campaign, &mut notes);
        super::rules::note_repetition_shortfall(&attributed, &mut capability_notes);
        workloads.insert(workload_id, attributed);
    }

    Ok(BenchmarkReport {
        campaign_id: campaign.id().to_owned(),
        currency: sheet.currency.clone(),
        retrieved: sheet.retrieved.clone(),
        workloads,
        capability_notes,
        comparability_problems,
        totals,
        subscription_micros,
    })
}

/// Returns the remote-execution duration of each attempt, keyed by its record.
///
/// A phase may be observed more than once in one attempt, so the segments are
/// summed. A run with no remote-execution phase is absent from the map, and the
/// caller falls back to the attempt's whole lifetime.
fn remote_execution_phases(campaign: &Campaign) -> BTreeMap<AttemptKey, u64> {
    let mut by_attempt: BTreeMap<AttemptKey, u64> = BTreeMap::new();
    for (_, phase) in &campaign.phases {
        if phase.phase != PhaseKind::RemoteExecution {
            continue;
        }
        let key = (phase.workload_id.clone(), phase.repetition);
        let total = by_attempt.entry(key).or_insert(0);
        *total = total.saturating_add(phase.duration_nanos);
    }
    by_attempt
}

/// The key a phase record shares with the run it belongs to.
type AttemptKey = (String, u32);

/// Attributes a share of the campaign's subscription to each group.
///
/// A plan is bought for the campaign, not for one workload, so the campaign
/// total is the figure that must be paid. A reader comparing two workloads
/// still needs to see what each contributed, so the subscription is divided
/// across the groups in proportion to their timed attempts, with the remainder
/// given to the first group so the shares always add back to the total.
fn attribute_subscription(
    mut results: Vec<WorkloadResult>,
    subscription_micros: u64,
) -> Vec<WorkloadResult> {
    let timed: u64 = results
        .iter()
        .map(|result| u64::from(result.completed.saturating_add(result.failed)))
        .sum();
    if timed == 0 || subscription_micros == 0 {
        return results;
    }
    let mut remaining = subscription_micros;
    let last = results.len().saturating_sub(1);
    for (index, result) in results.iter_mut().enumerate() {
        let weight = u64::from(result.completed.saturating_add(result.failed));
        let share = if index == last {
            remaining
        } else {
            let proportional = u128::from(subscription_micros)
                .saturating_mul(u128::from(weight))
                .checked_div(u128::from(timed))
                .unwrap_or(0);
            let share = u64::try_from(proportional).unwrap_or(u64::MAX);
            share.min(remaining)
        };
        remaining = remaining.saturating_sub(share);
        result.subscription_micros = share;
        result.total_account_micros = result.total_account_micros.saturating_add(share);
    }
    results
}
