//! Building a priced report from a validated campaign.
//!
//! Attempts are priced in the order the file records them, so a plan's
//! included compute is consumed by the earliest attempts. That ordering is why
//! a campaign file is append-only and why the builder takes the file's order
//! rather than re-sorting by duration.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    BenchmarkReport,
    ReportError,
    WorkloadResult,
    subscription::{PlanAllowances, attribute_subscription, remote_execution_phases},
};

/// The provider, product, cache class and role a group is keyed by.
type GroupKey = (String, String, CacheState, RunRole);
use crate::bench::{
    billing::{CostBreakdown, PriceSheet, RetainedStorage, Totals, Usage, charge},
    record::{CacheState, RunOutcome, RunRecord, RunRole},
    validate::Campaign,
};

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
    // A writer that already applied the provider's granularity and minimum
    // records a billed lifetime longer than the attempt; that figure is priced
    // directly. A record whose two lifetimes agree carries no billed figure, so
    // the sheet's rules are applied to the measured lifetime instead.
    let mut priced = Usage::new(
        run.provider.provider.clone(),
        run.provider.product.clone(),
        run.lifetime.total_nanos,
    );
    if run.lifetime.chargeable_nanos > run.lifetime.total_nanos {
        priced = priced.with_billed_lifetime(run.lifetime.chargeable_nanos);
    }
    if !run.retained.is_empty() {
        priced = priced.with_retained(
            run.retained
                .iter()
                .map(|item| RetainedStorage {
                    class: item.class.clone(),
                    bytes: item.bytes,
                    held_nanos: item.held_nanos,
                })
                .collect(),
        );
    }
    // The allowance is expressed in nanoseconds of *this* product, so a
    // product costing twice the plan's baseline absorbs twice the credit.
    let product = sheet
        .providers
        .get(&run.provider.provider)
        .and_then(|provider| provider.products.get(&run.provider.product))
        .ok_or_else(|| {
            ReportError::Billing(format!(
                "price sheet has no product {:?} for provider {:?}",
                run.provider.product, run.provider.provider
            ))
        })?;
    let usage = priced
        .with_plan_allowance(allowance.allowance_nanos(&run.provider.provider, product))
        .with_starts_used(allowance.starts_used(&run.provider.provider));
    let breakdown = charge(sheet, &usage).map_err(|err| ReportError::Billing(err.to_string()))?;
    allowance.consume(&run.provider.provider, product, breakdown.absorbed_nanos);
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
    /// Whether the group's attempts were timed or only warmed the cache.
    role: RunRole,
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
    /// Subscription share attributed to failed attempts.
    failed_subscription_micros: u64,
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
            role: run.role,
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
            failed_subscription_micros: 0,
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
                // The summary is about the workload, so it uses the remote-execution
                // phase only. An attempt without one has no measurement of the
                // workload itself, and mixing its whole lifetime into the same
                // series would make the statistic mean two different things.
                if let Some(duration) = remote_execution {
                    self.executions_nanos.push(*duration);
                }
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
            role: self.role,
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
            failed_subscription_micros: self.failed_subscription_micros,
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
/// An empty campaign is refused by the validator before a report is built, so
/// this example checks the validation step that must precede it:
///
/// ```
/// use mriya::bench::validate::validate_str;
///
/// let error = validate_str("").unwrap_err();
/// assert!(
///     error
///         .iter()
///         .any(|c| c.to_string().contains("no campaign header"))
/// );
/// ```
pub fn build_report(
    campaign: &Campaign,
    sheet: &PriceSheet,
) -> Result<BenchmarkReport, ReportError> {
    let mut allowance = PlanAllowances::new(sheet, &campaign.runs);
    let mut totals = Totals::default();
    let mut grouped: BTreeMap<String, BTreeMap<GroupKey, GroupBuilder>> = BTreeMap::new();

    let execution_phases = remote_execution_phases(campaign);
    for (_, run) in &campaign.runs {
        let breakdown = price_run(run, sheet, &mut allowance)?;
        breakdown
            .accumulate(&mut totals)
            .map_err(|err| ReportError::Billing(format!("campaign totals overflowed: {err}")))?;
        grouped
            .entry(run.workload_id.clone())
            .or_default()
            .entry((
                run.provider.provider.clone(),
                run.provider.product.clone(),
                run.cache.state,
                run.role,
            ))
            .or_insert_with(|| GroupBuilder::new(run))
            .push(run, &breakdown, execution_phases.get(&run.run_id));
    }

    // A plan is charged once for the campaign, not once per attempt. Adding it
    // after the per-run loop is what keeps the two figures distinct: the
    // marginal totals say what the gates cost, and the account total says what
    // the campaign adds to the bill.
    let subscriptions = super::provider_subscriptions(campaign, sheet)
        .map_err(|err| ReportError::Billing(err.to_string()))?;
    let subscription_micros: u64 = subscriptions.values().copied().sum();
    totals.subscription_micros = subscription_micros;
    totals.total_account_micros = totals
        .total_account_micros
        .saturating_add(subscription_micros);

    // Attribute the subscriptions once across the whole campaign, then group
    // the results by workload. Attributing per workload would charge each
    // provider's plan once for every workload it appears in.
    let mut flat: Vec<(String, WorkloadResult)> = grouped
        .into_iter()
        .flat_map(|(workload_id, groups)| {
            groups
                .into_values()
                .map(move |group| (workload_id.clone(), group.finish()))
        })
        .collect();
    attribute_subscription(&mut flat, &subscriptions);

    let by_workload = super::rules::attempts_by_workload(campaign);
    let mut workloads: BTreeMap<String, Vec<WorkloadResult>> = BTreeMap::new();
    let mut capability_notes = Vec::new();
    let mut incomplete_totals = Vec::new();
    note_unpriced_extras(campaign, sheet, &mut incomplete_totals);
    let mut upper_bounds = Vec::new();
    note_upper_bounds(campaign, sheet, &mut upper_bounds);
    let mut comparability_problems = Vec::new();
    for (workload_id, result) in flat {
        workloads.entry(workload_id).or_default().push(result);
    }
    note_workload_differences(
        &workloads,
        &by_workload,
        &mut capability_notes,
        &mut comparability_problems,
    );

    Ok(BenchmarkReport {
        campaign_id: campaign.id().to_owned(),
        currency: sheet.currency.clone(),
        retrieved: sheet.retrieved.clone(),
        workloads,
        capability_notes,
        incomplete_totals,
        upper_bounds,
        comparability_problems,
        totals,
        subscription_micros,
    })
}

/// Records each workload's capability differences and comparability problems.
fn note_workload_differences(
    workloads: &BTreeMap<String, Vec<WorkloadResult>>,
    by_workload: &BTreeMap<&str, Vec<&RunRecord>>,
    capability: &mut Vec<String>,
    comparability: &mut Vec<String>,
) {
    for (workload_id, results) in workloads {
        let attempts = by_workload
            .get(workload_id.as_str())
            .cloned()
            .unwrap_or_default();
        let mut notes = super::rules::DifferenceNotes {
            capability,
            comparability,
        };
        super::rules::note_differences(workload_id, results, &attempts, &mut notes);
        super::rules::note_repetition_shortfall(results, capability);
    }
}

/// States every charge a used provider bills that the sheet does not price.
///
/// The price path can only charge resources a record names. A provider that
/// bills something the record format has no unit for — an attached public
/// address, for example — would otherwise be silently understated, so the
/// report records why its totals are incomplete.
fn note_unpriced_extras(campaign: &Campaign, sheet: &PriceSheet, notes: &mut Vec<String>) {
    let providers: BTreeSet<&str> = campaign
        .runs
        .iter()
        .map(|(_, run)| run.provider.provider.as_str())
        .collect();
    for provider in providers {
        let Some(rules) = sheet.providers.get(provider) else {
            continue;
        };
        if rules.unpriced_extras.is_empty() {
            continue;
        }
        notes.push(format!(
            "{provider}: every total below excludes {}, which the price sheet does not price",
            rules.unpriced_extras.join(", ")
        ));
    }
}

/// States every used provider component whose price the sheet can only
/// overstate.
fn note_upper_bounds(campaign: &Campaign, sheet: &PriceSheet, notes: &mut Vec<String>) {
    let providers: BTreeSet<&str> = campaign
        .runs
        .iter()
        .map(|(_, run)| run.provider.provider.as_str())
        .collect();
    for provider in providers {
        let Some(rules) = sheet.providers.get(provider) else {
            continue;
        };
        for note in &rules.upper_bound_notes {
            notes.push(format!("{provider}: {note}"));
        }
    }
}
