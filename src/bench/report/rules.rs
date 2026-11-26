//! The comparison rules the report enforces.
//!
//! Equal work, stated capability differences, and a subscription charged once
//! per campaign rather than once per attempt. These are the rules that decide
//! whether two numbers may be printed side by side, so they live together and
//! are tested as policy rather than as formatting.

use std::collections::BTreeSet;

use super::WorkloadResult;
use crate::bench::{
    billing::{MICROS_PER_UNIT, PriceSheet},
    record::{CacheState, WorkloadClass},
    validate::Campaign,
};

/// The two note lists a group's differences are recorded in.
pub(super) struct DifferenceNotes<'a> {
    /// Capability differences the reader must see before comparing.
    pub(super) capability: &'a mut Vec<String>,
    /// Problems that prevent a comparison between providers.
    pub(super) comparability: &'a mut Vec<String>,
}

/// Records the capability differences and comparability problems in a group.
pub(super) fn note_differences(
    workload_id: &str,
    results: &[WorkloadResult],
    campaign: &Campaign,
    notes: &mut DifferenceNotes<'_>,
) {
    let capability_notes = &mut *notes.capability;
    let comparability_problems = &mut *notes.comparability;
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
            .checked_div(u128::from(MICROS_PER_UNIT))
            .unwrap_or(0);
        total = total.saturating_add(u64::try_from(converted).unwrap_or(u64::MAX));
    }
    total
}
