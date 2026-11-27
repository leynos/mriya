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

mod build;
mod render;
mod rules;
mod subscription;

pub use render::{format_duration, render};
pub use rules::{campaign_subscription_micros, provider_subscriptions};

use super::{
    billing::{PriceSheet, Totals},
    record::CacheState,
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
    /// Cache class the group's first attempt started from.
    pub cache_state: CacheState,
    /// Every cache class the group's attempts declared.
    ///
    /// More than one means the repetitions were not comparable with each
    /// other, which the report states rather than hiding behind the first
    /// attempt's class.
    pub cache_states: BTreeSet<CacheState>,
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
    /// Marginal cost of the cache warm-up attempts, excluded from timing.
    pub warm_up_marginal_micros: u64,
    /// Subscription share attributed to the completed attempts.
    pub subscription_micros: u64,
    /// Subscription share attributed to the failed attempts.
    ///
    /// Reported beside the failed-attempt cost, so a plan is never presented as
    /// a cost of completed work when nothing completed.
    pub failed_subscription_micros: u64,
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
    ///     cache_states: std::collections::BTreeSet::from([CacheState::Empty]),
    ///     cache_bytes: 0,
    ///     completed: 2,
    ///     failed: 0,
    ///     executions_nanos: vec![1, 2],
    ///     completed_marginal_micros: 3,
    ///     failed_marginal_micros: 0,
    ///     warm_up_marginal_micros: 0,
    ///     subscription_micros: 0,
    ///     failed_subscription_micros: 0,
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
            .checked_div(u64::from(self.completed))
            .map(|quotient| {
                let remainder = self
                    .completed_marginal_micros
                    .checked_rem(u64::from(self.completed))
                    .unwrap_or(0);
                if remainder == 0 {
                    quotient
                } else {
                    quotient.saturating_add(1)
                }
            })
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
    ///     cache_states: std::collections::BTreeSet::from([CacheState::Empty]),
    ///     cache_bytes: 0,
    ///     completed: 1,
    ///     failed: 0,
    ///     executions_nanos: vec![7, 3],
    ///     completed_marginal_micros: 0,
    ///     failed_marginal_micros: 0,
    ///     warm_up_marginal_micros: 0,
    ///     subscription_micros: 0,
    ///     failed_subscription_micros: 0,
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
    /// Why the totals are incomplete, when a used provider bills a charge the
    /// price sheet does not price.
    pub incomplete_totals: Vec<String>,
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
/// Attempts are priced in the order the file records them, so a plan's
/// included compute is consumed by the earliest attempts.
///
/// # Errors
///
/// Returns [`ReportError`] when a run cannot be priced against the sheet, or
/// when its recorded lifetime contradicts its own measurement.
///
/// # Examples
///
/// An empty campaign is refused by the validator before a report is built:
///
/// ```
/// use mriya::bench::validate::validate_str;
///
/// assert!(validate_str("").is_err());
/// ```
pub fn build_report(
    campaign: &Campaign,
    sheet: &PriceSheet,
) -> Result<BenchmarkReport, ReportError> {
    build::build_report(campaign, sheet)
}
