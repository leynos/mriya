//! Attribution of phases and subscription charges to a report's groups.
//!
//! A report knows what each attempt cost, but two things need distributing
//! before a reader can compare workloads: each attempt's remote-execution
//! timing, and each provider's monthly plan.

use std::collections::BTreeMap;

use super::WorkloadResult;
use crate::bench::{
    billing::{PriceSheet, ProductRule},
    record::{PhaseKind, RunRecord},
    validate::Campaign,
};

/// Returns the remote-execution duration of each attempt, keyed by its record.
///
/// A phase may be observed more than once in one attempt, so the segments are
/// summed. A run with no remote-execution phase is absent from the map, and the
/// caller falls back to the attempt's whole lifetime.
pub(super) fn remote_execution_phases(campaign: &Campaign) -> BTreeMap<String, u64> {
    let mut by_attempt: BTreeMap<String, u64> = BTreeMap::new();
    for (_, phase) in &campaign.phases {
        if phase.phase != PhaseKind::RemoteExecution {
            continue;
        }
        let total = by_attempt.entry(phase.run_id.clone()).or_insert(0);
        *total = total.saturating_add(phase.duration_nanos);
    }
    by_attempt
}

/// Attributes a share of each provider's subscription to the groups.
///
/// A plan is bought once per provider for the campaign, so the campaign total
/// is the figure that must be paid. A reader comparing workloads still needs to
/// see what each contributed, so each provider's subscription is divided across
/// that provider's own groups in proportion to their timed attempts. The
/// remainder goes to the provider's last group in the first workload it
/// appears in, so the shares always add back to the provider's total and no
/// provider is charged another's plan.
pub(super) fn attribute_subscription(
    results: &mut [(String, WorkloadResult)],
    subscriptions: &BTreeMap<String, u64>,
) {
    // Only groups with timed attempts can carry a share: a plan is bought for
    // the work, and a provider whose campaign held only warm-ups has no timed
    // work to attribute it to.
    let weights: Vec<u64> = results
        .iter()
        .map(|(_, result)| u64::from(result.completed.saturating_add(result.failed)))
        .collect();
    let mut provider_weight: BTreeMap<String, u64> = BTreeMap::new();
    for ((_, result), weight) in results.iter().zip(&weights) {
        *provider_weight.entry(result.provider.clone()).or_insert(0) += weight;
    }
    // A per-index copy of the provider and weight, so the loop can borrow the
    // results mutably while still looking ahead.
    let providers: Vec<(String, u64)> = results
        .iter()
        .zip(&weights)
        .map(|((_, result), weight)| (result.provider.clone(), *weight))
        .collect();
    let mut remaining = subscriptions.clone();
    for (index, ((_, result), weight)) in
        results.iter_mut().zip(weights.iter().copied()).enumerate()
    {
        if weight == 0 {
            continue;
        }
        let Some(total) = subscriptions.get(&result.provider).copied() else {
            continue;
        };
        if total == 0 {
            continue;
        }
        let whole_provider = provider_weight
            .get(&result.provider)
            .copied()
            .unwrap_or(0)
            .max(1);
        let left = remaining.get(&result.provider).copied().unwrap_or(0);
        // The last weighted group of a provider absorbs the rounding
        // remainder, so the shares add back to that provider's total.
        let is_last_weighted = !providers
            .iter()
            .skip(index.saturating_add(1))
            .any(|(provider, later_weight)| *later_weight > 0 && *provider == result.provider);
        let share = if is_last_weighted {
            left
        } else {
            let proportional = u128::from(total)
                .saturating_mul(u128::from(weight))
                .checked_div(u128::from(whole_provider))
                .unwrap_or(0);
            u64::try_from(proportional).unwrap_or(u64::MAX).min(left)
        };
        remaining.insert(result.provider.clone(), left.saturating_sub(share));
        // Only the completed attempts' share belongs in the completed-gate
        // figures; a group with no completed attempt must not report a
        // completed-gate cost.
        let weight_total = result.completed.saturating_add(result.failed);
        let completed_share = if weight_total == 0 {
            0
        } else {
            let proportional = u128::from(share)
                .saturating_mul(u128::from(result.completed))
                .checked_div(u128::from(weight_total))
                .unwrap_or(0);
            u64::try_from(proportional).unwrap_or(u64::MAX)
        };
        result.subscription_micros = completed_share;
        result.failed_subscription_micros = share.saturating_sub(completed_share);
        result.total_account_micros = result.total_account_micros.saturating_add(completed_share);
    }
}

/// Tracks how much of each provider plan's included compute remains.
///
/// The balance is money in the provider's own currency, not hours: a plan
/// grants a value of machine time, and an hour of a product that costs twice
/// the baseline consumes twice as much of it.
pub(super) struct PlanAllowances {
    /// Remaining included value per provider, in its own currency's micros.
    remaining_micros: BTreeMap<String, u64>,
    /// Starts already used per provider.
    starts_used: BTreeMap<String, u64>,
}

impl PlanAllowances {
    /// Reads each provider's plan from the sheet and values its allowance.
    pub(super) fn new(sheet: &PriceSheet, runs: &[(usize, RunRecord)]) -> Self {
        let mut remaining = BTreeMap::new();
        for (_, run) in runs {
            let provider_name = &run.provider.provider;
            if remaining.contains_key(provider_name) {
                continue;
            }
            let Some(provider) = sheet.providers.get(provider_name) else {
                continue;
            };
            let Some(plan) = provider.plan.as_ref() else {
                continue;
            };
            let Some(rule) = provider.products.get(&plan.baseline_product) else {
                continue;
            };
            // Included hours times the baseline rate is the plan's allowance in
            // the provider's currency.
            let credit = u128::from(plan.included_compute_nanos)
                .saturating_mul(u128::from(rule.price_per_hour_micros))
                .checked_div(u128::from(crate::bench::billing::NANOS_PER_HOUR))
                .unwrap_or(0);
            remaining.insert(
                provider_name.clone(),
                u64::try_from(credit).unwrap_or(u64::MAX),
            );
        }
        Self {
            remaining_micros: remaining,
            starts_used: BTreeMap::new(),
        }
    }

    /// Returns the allowance left for `provider` expressed in nanoseconds of
    /// `product`.
    ///
    /// A product that costs nothing has no meaningful allowance; the caller then
    /// passes zero, and the plan absorbs nothing.
    pub(super) fn allowance_nanos(&self, provider: &str, product: &ProductRule) -> u64 {
        let credit = self.remaining_micros.get(provider).copied().unwrap_or(0);
        if credit == 0 || product.price_per_hour_micros == 0 {
            return 0;
        }
        let nanos = u128::from(credit)
            .saturating_mul(u128::from(crate::bench::billing::NANOS_PER_HOUR))
            .checked_div(u128::from(product.price_per_hour_micros))
            .unwrap_or(0);
        u64::try_from(nanos).unwrap_or(u64::MAX)
    }

    /// Returns how many starts had been used before this gate.
    pub(super) fn starts_used(&self, provider: &str) -> u64 {
        self.starts_used.get(provider).copied().unwrap_or(0)
    }

    /// Consumes the value of `nanos` of `product` from the provider's credit.
    pub(super) fn consume(&mut self, provider: &str, product: &ProductRule, nanos: u64) {
        let Some(spent) = product.price_micros(nanos) else {
            return;
        };
        if let Some(remaining) = self.remaining_micros.get_mut(provider) {
            *remaining = remaining.saturating_sub(spent);
        }
        let starts = self.starts_used.entry(provider.to_owned()).or_insert(0);
        *starts = starts.saturating_add(1);
    }
}
