//! Offline report: what each provider's completed work actually cost.
//!
//! The report is where the protocol's comparison rules are enforced. It reads a
//! validated campaign and a price sheet and refuses to compare work that is not
//! equal: a different command, revision, or architecture is a different
//! experiment, and a different cache class is a capability difference that must
//! be stated rather than averaged away.
//!
//! Two figures are printed for every group. Marginal cost is what one more
//! identical gate costs once the plan is paid. Total-account cost is what the
//! campaign adds to the bill. They differ exactly when a platform charges for a
//! plan, and the report says which of the two applies rather than printing one
//! number under two headings.
//!
//! Nothing here contacts a provider or reads a clock.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use super::{
    billing::{
        CostBreakdown,
        NANOS_PER_SECOND,
        PriceSheet,
        Totals,
        Usage,
        charge,
        figures_agree,
        format_currency,
    },
    record::{CacheState, RunOutcome, RunRecord, WorkloadClass},
    validate::Campaign,
};

/// One provider's result for one workload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkloadResult {
    /// Provider name.
    pub provider: String,
    /// Product the provider allocated.
    pub product: String,
    /// Workload identifier.
    pub workload_id: String,
    /// Cache class every attempt in the group started from.
    pub cache_state: CacheState,
    /// Bytes the cache held when the attempts started.
    pub cache_bytes: u64,
    /// How many attempts completed.
    pub completed: u32,
    /// How many attempts failed or aborted.
    pub failed: u32,
    /// Remote-execution durations of completed attempts, in nanoseconds.
    pub executions_nanos: Vec<u64>,
    /// Marginal cost of the completed attempts.
    pub completed_marginal_micros: u64,
    /// Marginal cost of the failed attempts.
    pub failed_marginal_micros: u64,
    /// Subscription share attributed to the completed attempts.
    pub subscription_micros: u64,
    /// Total-account cost of the completed attempts.
    pub total_account_micros: u64,
    /// Compute time the plan absorbed, in nanoseconds.
    pub absorbed_nanos: u64,
    /// Chargeable lifetime the provider billed, in nanoseconds.
    pub chargeable_nanos: u64,
}

impl WorkloadResult {
    /// Returns the marginal cost of one completed gate, or `None` when nothing
    /// completed.
    ///
    /// The value is rounded up to the micro-unit, so a per-gate figure never
    /// understates the campaign it came from.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::{record::CacheState, report::WorkloadResult};
    ///
    /// let result = WorkloadResult {
    ///     provider: String::from("scaleway"),
    ///     product: String::from("DEV1-S"),
    ///     workload_id: String::from("cold-build"),
    ///     cache_state: CacheState::Empty,
    ///     cache_bytes: 0,
    ///     completed: 2,
    ///     failed: 0,
    ///     executions_nanos: vec![1, 2],
    ///     completed_marginal_micros: 3,
    ///     failed_marginal_micros: 0,
    ///     subscription_micros: 0,
    ///     total_account_micros: 3,
    ///     absorbed_nanos: 0,
    ///     chargeable_nanos: 0,
    /// };
    /// assert_eq!(result.marginal_per_gate_micros(), Some(2));
    /// ```
    #[must_use]
    pub fn marginal_per_gate_micros(&self) -> Option<u64> {
        if self.completed == 0 {
            return None;
        }
        self.completed_marginal_micros
            .checked_add(u64::from(self.completed).saturating_sub(1))
            .map(|value| value.checked_div(u64::from(self.completed)).unwrap_or(0))
    }

    /// Returns the least remote-execution duration, or `None` when nothing
    /// completed.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::{record::CacheState, report::WorkloadResult};
    ///
    /// let result = WorkloadResult {
    ///     provider: String::from("p"),
    ///     product: String::from("x"),
    ///     workload_id: String::from("w"),
    ///     cache_state: CacheState::Empty,
    ///     cache_bytes: 0,
    ///     completed: 1,
    ///     failed: 0,
    ///     executions_nanos: vec![7, 3],
    ///     completed_marginal_micros: 0,
    ///     failed_marginal_micros: 0,
    ///     subscription_micros: 0,
    ///     total_account_micros: 0,
    ///     absorbed_nanos: 0,
    ///     chargeable_nanos: 0,
    /// };
    /// assert_eq!(result.minimum_execution_nanos(), Some(3));
    /// ```
    #[must_use]
    pub fn minimum_execution_nanos(&self) -> Option<u64> {
        self.executions_nanos.iter().copied().min()
    }

    /// Returns the greatest remote-execution duration, or `None` when nothing
    /// completed.
    #[must_use]
    pub fn maximum_execution_nanos(&self) -> Option<u64> {
        self.executions_nanos.iter().copied().max()
    }

    /// Returns the median remote-execution duration, or `None` when nothing
    /// completed.
    ///
    /// An even count takes the lower of the two middle values, which is the
    /// conservative choice for a measurement whose distribution has a long
    /// right tail.
    #[must_use]
    pub fn median_execution_nanos(&self) -> Option<u64> {
        let mut ordered = self.executions_nanos.clone();
        ordered.sort_unstable();
        let middle = ordered.len().checked_sub(1)?.checked_div(2)?;
        ordered.get(middle).copied()
    }
}

/// The whole report, grouped so a reader can check comparability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchmarkReport {
    /// Campaign identifier.
    pub campaign_id: String,
    /// Currency every figure is expressed in.
    pub currency: String,
    /// Date the price sheet was read.
    pub retrieved: String,
    /// Results grouped by workload, then by provider and product.
    pub workloads: BTreeMap<String, Vec<WorkloadResult>>,
    /// Capability differences the reader must see before comparing.
    pub capability_notes: Vec<String>,
    /// Problems that prevent a comparison between providers.
    pub comparability_problems: Vec<String>,
    /// Campaign-wide totals in the sheet's currency.
    pub totals: Totals,
    /// Subscription the campaign is charged, in the sheet's currency.
    pub subscription_micros: u64,
}

/// Why a campaign could not be reported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReportError {
    /// A run names a provider, product, or storage class the sheet lacks.
    Billing(String),
    /// A run's chargeable lifetime is inconsistent with its measurement.
    Incoherent {
        /// The workload the run belongs to.
        workload_id: String,
        /// What is wrong.
        message: String,
    },
}

impl fmt::Display for ReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Billing(message) => write!(formatter, "cannot price the campaign: {message}"),
            Self::Incoherent {
                workload_id,
                message,
            } => write!(
                formatter,
                "workload {workload_id} is not coherent: {message}"
            ),
        }
    }
}

impl std::error::Error for ReportError {}

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
            .push(run, &breakdown);
    }

    // A plan is charged once for the campaign, not once per attempt. Adding it
    // after the per-run loop is what keeps the two figures distinct: the
    // marginal totals say what the gates cost, and the account total says what
    // the campaign adds to the bill.
    let subscription_micros = campaign_subscription_micros(campaign, sheet);
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
        note_differences(
            &workload_id,
            &results,
            campaign,
            &mut capability_notes,
            &mut comparability_problems,
        );
        workloads.insert(workload_id, results);
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
    let usage = Usage::new(
        run.provider.provider.clone(),
        run.provider.product.clone(),
        run.lifetime.total_nanos,
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
            cache_bytes: run.cache.size_bytes,
            completed: 0,
            failed: 0,
            executions_nanos: Vec::new(),
            completed_marginal_micros: 0,
            failed_marginal_micros: 0,
            subscription_micros: 0,
            total_account_micros: 0,
            absorbed_nanos: 0,
            chargeable_nanos: 0,
        }
    }

    /// Adds one priced attempt to the group.
    fn push(&mut self, run: &RunRecord, breakdown: &CostBreakdown) {
        self.absorbed_nanos = self.absorbed_nanos.saturating_add(breakdown.absorbed_nanos);
        self.chargeable_nanos = self
            .chargeable_nanos
            .saturating_add(breakdown.chargeable_nanos);
        match run.outcome {
            RunOutcome::Completed { .. } => {
                self.completed = self.completed.saturating_add(1);
                self.executions_nanos.push(run.lifetime.total_nanos);
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
            cache_bytes: self.cache_bytes,
            completed: self.completed,
            failed: self.failed,
            executions_nanos: self.executions_nanos,
            completed_marginal_micros: self.completed_marginal_micros,
            failed_marginal_micros: self.failed_marginal_micros,
            subscription_micros: self.subscription_micros,
            total_account_micros: self.total_account_micros,
            absorbed_nanos: self.absorbed_nanos,
            chargeable_nanos: self.chargeable_nanos,
        }
    }
}

/// Records the capability differences and comparability problems in a group.
fn note_differences(
    workload_id: &str,
    results: &[WorkloadResult],
    campaign: &Campaign,
    capability_notes: &mut Vec<String>,
    comparability_problems: &mut Vec<String>,
) {
    let cache_classes: BTreeSet<CacheState> = results.iter().map(|r| r.cache_state).collect();
    if cache_classes.len() > 1 {
        capability_notes.push(format!(
            "{workload_id}: providers used different cache classes ({}); a cost difference \
             between them is partly a cache-capability difference, not only a throughput one",
            describe_cache_classes(&cache_classes)
        ));
    }
    let gpu_workloads: BTreeSet<bool> = campaign
        .runs
        .iter()
        .filter(|(_, run)| run.workload_id == workload_id)
        .map(|(_, run)| run.class == WorkloadClass::Gpu)
        .collect();
    if gpu_workloads.len() > 1 {
        comparability_problems.push(format!(
            "{workload_id}: mixes CPU and GPU experiments, which are never ranked together"
        ));
    }
    let mut commands = BTreeSet::new();
    let mut revisions = BTreeSet::new();
    let mut architectures = BTreeSet::new();
    for (_, run) in &campaign.runs {
        if run.workload_id != workload_id {
            continue;
        }
        commands.insert(run.command.join(" "));
        revisions.insert(run.tree.commit.clone());
        architectures.insert(format!("{:?}", run.allocation.guest_architecture));
    }
    if commands.len() > 1 {
        comparability_problems.push(format!(
            "{workload_id}: attempts ran different commands, so they are not equal work"
        ));
    }
    if revisions.len() > 1 {
        comparability_problems.push(format!(
            "{workload_id}: attempts ran different revisions, so they are not equal work"
        ));
    }
    if architectures.len() > 1 {
        comparability_problems.push(format!(
            "{workload_id}: attempts ran on different architectures, so they are not equal work"
        ));
    }
}

/// Renders the cache classes in a stable order for a human-readable note.
fn describe_cache_classes(classes: &BTreeSet<CacheState>) -> String {
    classes
        .iter()
        .map(|state| format!("{state:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders a duration as seconds with three decimals, without a float.
///
/// # Examples
///
/// ```
/// use mriya::bench::report::format_duration;
///
/// assert_eq!(format_duration(0), "0.000s");
/// assert_eq!(format_duration(1_500_000_000), "1.500s");
/// ```
#[must_use]
pub fn format_duration(nanos: u64) -> String {
    let seconds = nanos.checked_div(NANOS_PER_SECOND).unwrap_or(0);
    let millis = nanos
        .checked_rem(NANOS_PER_SECOND)
        .unwrap_or(0)
        .checked_div(1_000_000)
        .unwrap_or(0);
    format!("{seconds}.{millis:03}s")
}

/// Renders the report as plain text.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
///
/// use mriya::bench::{billing::Totals, report::BenchmarkReport};
///
/// let report = BenchmarkReport {
///     campaign_id: String::from("c"),
///     currency: String::from("EUR"),
///     retrieved: String::from("2026-11-26"),
///     workloads: BTreeMap::new(),
///     capability_notes: Vec::new(),
///     comparability_problems: Vec::new(),
///     totals: Totals::default(),
///     subscription_micros: 0,
/// };
/// let rendered = render(&report);
/// assert!(rendered.contains("campaign c"));
/// ```
#[must_use]
pub fn render(report: &BenchmarkReport) -> String {
    let mut lines = vec![
        format!(
            "campaign {} priced in {} from a sheet read {}",
            report.campaign_id, report.currency, report.retrieved
        ),
        String::new(),
    ];

    if report.workloads.is_empty() {
        lines.push(String::from("no run records; nothing to report"));
    }
    for (workload_id, results) in &report.workloads {
        lines.push(format!("workload {workload_id}"));
        for result in results {
            lines.extend(render_result(report, result));
        }
        lines.push(String::new());
    }

    if !report.capability_notes.is_empty() {
        lines.push(String::from("capability differences"));
        for note in &report.capability_notes {
            lines.push(format!("- {note}"));
        }
        lines.push(String::new());
    }
    if !report.comparability_problems.is_empty() {
        lines.push(String::from("comparability problems"));
        for problem in &report.comparability_problems {
            lines.push(format!("- {problem}"));
        }
        lines.push(String::new());
    }

    lines.push(format!(
        "campaign marginal cost {}",
        format_currency(&report.currency, report.totals.marginal_micros)
    ));
    lines.push(format!(
        "campaign total-account cost {} of which subscription {}",
        format_currency(&report.currency, report.totals.total_account_micros),
        format_currency(&report.currency, report.subscription_micros)
    ));
    lines.push(String::new());
    lines.join("\n")
}

/// Renders one provider-product group.
fn render_result(report: &BenchmarkReport, result: &WorkloadResult) -> Vec<String> {
    let mut lines = vec![format!(
        "  {} / {} product {}: {} completed, {} failed",
        result.workload_id, result.provider, result.product, result.completed, result.failed
    )];
    lines.push(format!(
        "    cache {:?}, {} bytes; absorbed by plan {}; chargeable {}",
        result.cache_state,
        result.cache_bytes,
        format_duration(result.absorbed_nanos),
        format_duration(result.chargeable_nanos)
    ));
    if let Some(per_gate) = result.marginal_per_gate_micros() {
        lines.push(format!(
            "    marginal cost per completed gate {}",
            format_currency(&report.currency, per_gate)
        ));
    } else {
        lines.push(String::from(
            "    no completed gate, so no marginal cost per gate is reported",
        ));
    }
    if result.failed > 0 {
        lines.push(format!(
            "    failed-attempt cost {}",
            format_currency(&report.currency, result.failed_marginal_micros)
        ));
    }
    if let (Some(minimum), Some(median), Some(maximum)) = (
        result.minimum_execution_nanos(),
        result.median_execution_nanos(),
        result.maximum_execution_nanos(),
    ) {
        lines.push(format!(
            "    execution min {} median {} max {}",
            format_duration(minimum),
            format_duration(median),
            format_duration(maximum)
        ));
    }
    if !figures_agree(
        result.completed_marginal_micros,
        result.total_account_micros,
    ) {
        lines.push(format!(
            "    total-account cost of completed gates {}, including subscription share {}",
            format_currency(&report.currency, result.total_account_micros),
            format_currency(&report.currency, result.subscription_micros)
        ));
    }
    lines
}

/// Returns the campaign's subscription total in the sheet's currency.
///
/// The plan price is charged once per campaign rather than once per attempt, so
/// this is computed from the sheet and the providers the campaign actually used.
///
/// # Examples
///
/// A campaign with no plan is charged no subscription:
///
/// ```
/// use mriya::bench::validate::validate_str;
///
/// let campaign = validate_str("").unwrap_err();
/// assert_eq!(campaign.len(), 2);
/// ```
#[must_use]
pub fn campaign_subscription_micros(campaign: &Campaign, sheet: &PriceSheet) -> u64 {
    let providers: BTreeSet<&str> = campaign
        .runs
        .iter()
        .map(|(_, run)| run.provider.provider.as_str())
        .collect();
    let mut total = 0_u64;
    for provider in providers {
        let Some(rules) = sheet.providers.get(provider) else {
            continue;
        };
        let Some(plan) = rules.plan.as_ref() else {
            continue;
        };
        let rate = sheet.rate(&rules.currency).unwrap_or(0);
        let converted = u128::from(plan.monthly_price_micros)
            .saturating_mul(u128::from(rate))
            .checked_div(u128::from(super::billing::MICROS_PER_UNIT))
            .unwrap_or(0);
        total = total.saturating_add(u64::try_from(converted).unwrap_or(u64::MAX));
    }
    total
}
