//! Attribution of phases and subscription charges to a report's groups.
//!
//! A report knows what each attempt cost, but two things need distributing
//! before a reader can compare workloads: each attempt's remote-execution
//! timing, and each provider's monthly plan.

use std::collections::BTreeMap;

use super::WorkloadResult;
use crate::bench::{record::PhaseKind, validate::Campaign};

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
    mut results: Vec<WorkloadResult>,
    subscriptions: &BTreeMap<String, u64>,
) -> Vec<WorkloadResult> {
    let mut remaining = subscriptions.clone();
    let mut seen: BTreeMap<String, u64> = BTreeMap::new();
    for result in &results {
        let weight = u64::from(result.completed.saturating_add(result.failed));
        *seen.entry(result.provider.clone()).or_insert(0) += weight;
    }
    let last_index = results.len().saturating_sub(1);
    // Which provider each later group belongs to, so the last group of each
    // provider can absorb that provider's remainder.
    let later: Vec<String> = results.iter().map(|r| r.provider.clone()).collect();
    for (index, result) in results.iter_mut().enumerate() {
        let Some(total) = subscriptions.get(&result.provider).copied() else {
            continue;
        };
        if total == 0 {
            continue;
        }
        let weight = u64::from(result.completed.saturating_add(result.failed));
        let provider_total_weight = seen.get(&result.provider).copied().unwrap_or(0);
        let left = remaining.get(&result.provider).copied().unwrap_or(0);
        let is_last_for_provider = index == last_index
            || !later
                .iter()
                .skip(index.saturating_add(1))
                .any(|later_provider| *later_provider == result.provider);
        let share = if is_last_for_provider || provider_total_weight == 0 {
            left
        } else {
            let proportional = u128::from(total)
                .saturating_mul(u128::from(weight))
                .checked_div(u128::from(provider_total_weight))
                .unwrap_or(0);
            u64::try_from(proportional).unwrap_or(u64::MAX).min(left)
        };
        remaining.insert(result.provider.clone(), left.saturating_sub(share));
        result.subscription_micros = share;
        result.total_account_micros = result.total_account_micros.saturating_add(share);
    }
    results
}
