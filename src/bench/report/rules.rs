//! The comparison rules the report enforces.
//!
//! Equal work, stated capability differences, and a subscription charged once
//! per campaign rather than once per attempt. These are the rules that decide
//! whether two numbers may be printed side by side, so they live together and
//! are tested as policy rather than as formatting.

use std::collections::{BTreeMap, BTreeSet};

use super::WorkloadResult;
use crate::bench::{
    billing::{PriceSheet, convert_micros},
    record::{CacheState, GuestArchitecture, RunRecord, WorkloadClass},
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
    attempts: &[&RunRecord],
    notes: &mut DifferenceNotes<'_>,
) {
    note_cache_differences(workload_id, results, notes.capability);
    note_comparability(attempts, workload_id, notes.comparability);
}

/// Groups a campaign's run records by the workload they belong to.
///
/// Built once and passed to each workload's rule, so the report does not scan
/// every run once per workload.
pub(super) fn attempts_by_workload(campaign: &Campaign) -> BTreeMap<&str, Vec<&RunRecord>> {
    let mut grouped: BTreeMap<&str, Vec<&RunRecord>> = BTreeMap::new();
    for (_, run) in &campaign.runs {
        grouped
            .entry(run.workload_id.as_str())
            .or_default()
            .push(run);
    }
    grouped
}

/// The completed repetitions the protocol requires before a claim is made.
pub const MIN_COMPLETED_REPETITIONS: u32 = 5;

/// States how far short of the required repetition count a group fell.
///
/// The protocol requires at least five completed attempts before a per-gate
/// figure may be published. A report can still show a smaller campaign — it is
/// how an operator checks a protocol before spending on a full one — but it
/// says so, rather than letting a single sample read as evidence.
pub(super) fn note_repetition_shortfall(results: &[WorkloadResult], notes: &mut Vec<String>) {
    for result in results {
        if result.completed < MIN_COMPLETED_REPETITIONS {
            notes.push(format!(
                "{}: {} completed repetitions, fewer than the {} the protocol requires before a \
                 claim; this is a dry run, not evidence",
                result.workload_id, result.completed, MIN_COMPLETED_REPETITIONS
            ));
        }
    }
}

/// States every cache class the group's attempts used, when they differ.
fn note_cache_differences(
    workload_id: &str,
    results: &[WorkloadResult],
    capability_notes: &mut Vec<String>,
) {
    let mut cache_classes = BTreeSet::new();
    for result in results {
        cache_classes.extend(result.cache_states.iter().copied());
    }
    if cache_classes.len() > 1 {
        capability_notes.push(format!(
            "{workload_id}: attempts used different cache classes ({}); a cost difference \
             between them is partly a cache-capability difference, not only a throughput one",
            describe_cache_classes(&cache_classes)
        ));
    }
}

/// Reports every way the group's attempts are not equal work.
fn note_comparability(attempts: &[&RunRecord], workload_id: &str, problems: &mut Vec<String>) {
    let classes: BTreeSet<bool> = attempts
        .iter()
        .map(|run| run.class == WorkloadClass::Gpu)
        .collect();
    if classes.len() > 1 {
        problems.push(format!(
            "{workload_id}: mixes CPU and GPU experiments, which are never ranked together"
        ));
    }
    // The argument vectors are compared whole, not joined: `["cargo",
    // "build x"]` and `["cargo build", "x"]` are different commands that a
    // join would make identical.
    let commands: BTreeSet<&Vec<String>> = attempts.iter().map(|run| &run.command).collect();
    if commands.len() > 1 {
        problems.push(format!(
            "{workload_id}: attempts ran different commands, so they are not equal work"
        ));
    }
    let revisions: BTreeSet<&str> = attempts
        .iter()
        .map(|run| run.tree.commit.as_str())
        .collect();
    if revisions.len() > 1 {
        problems.push(format!(
            "{workload_id}: attempts ran different revisions, so they are not equal work"
        ));
    }
    let digests: BTreeSet<&str> = attempts
        .iter()
        .map(|run| run.tree.tree_digest.as_str())
        .collect();
    if digests.len() > 1 {
        problems.push(format!(
            "{workload_id}: attempts ran different tree digests, so they are not equal work"
        ));
    }
    let cleanliness: BTreeSet<bool> = attempts.iter().map(|run| run.tree.dirty).collect();
    if cleanliness.len() > 1 {
        problems.push(format!(
            "{workload_id}: attempts mixed dirty and clean trees, which are not comparable"
        ));
    }
    let architectures: BTreeSet<GuestArchitecture> = attempts
        .iter()
        .map(|run| run.allocation.guest_architecture)
        .collect();
    if architectures.len() > 1 {
        problems.push(format!(
            "{workload_id}: attempts ran on different architectures, so they are not equal work"
        ));
    }
}

/// Renders the cache classes in a stable order for a human-readable note.
fn describe_cache_classes(classes: &BTreeSet<CacheState>) -> String {
    classes
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Returns the campaign's subscription total in the sheet's currency.
///
/// The plan price is charged once per campaign rather than once per attempt, so
/// this is computed from the sheet and the providers the campaign actually
/// used. The conversion uses the same rounding as every other money figure, so
/// a subscription cannot be understated by truncation.
///
/// # Errors
///
/// Returns a message when a provider quotes in a currency the sheet cannot
/// convert, rather than silently charging zero.
///
/// # Examples
///
/// ```
/// # use std::collections::BTreeMap;
/// # use mriya::bench::billing::PriceSheet;
/// # use mriya::bench::record::{CampaignHeader, RecordKind, SCHEMA_VERSION};
/// # use mriya::bench::report::campaign_subscription_micros;
/// # use mriya::bench::validate::Campaign;
/// let campaign = Campaign {
///     header: CampaignHeader {
///         schema: SCHEMA_VERSION,
///         kind: RecordKind::CampaignHeader,
///         campaign_id: String::from("c"),
///         limits: None,
///         price_sheet: None,
///         providers: Vec::new(),
///     },
///     runs: Vec::new(),
///     phases: Vec::new(),
/// };
/// let sheet = PriceSheet {
///     currency: String::from("EUR"),
///     retrieved: String::from("2026-11-26"),
///     month_seconds: 2_592_000,
///     providers: BTreeMap::new(),
///     fx: BTreeMap::new(),
/// };
/// assert_eq!(campaign_subscription_micros(&campaign, &sheet)?, 0);
/// # Ok::<(), String>(())
/// ```
pub fn campaign_subscription_micros(
    campaign: &Campaign,
    sheet: &PriceSheet,
) -> Result<u64, String> {
    Ok(provider_subscriptions(campaign, sheet)?
        .values()
        .copied()
        .sum())
}

/// Returns each provider's own subscription charge, in the sheet's currency.
///
/// One entry per provider that has a plan and at least one run, so a caller can
/// attribute each plan to the provider that charges it.
///
/// # Errors
///
/// Returns a message when a provider quotes in a currency the sheet cannot
/// convert.
pub fn provider_subscriptions(
    campaign: &Campaign,
    sheet: &PriceSheet,
) -> Result<BTreeMap<String, u64>, String> {
    let providers: BTreeSet<&str> = campaign
        .runs
        .iter()
        .map(|(_, run)| run.provider.provider.as_str())
        .collect();
    let mut charges = BTreeMap::new();
    for provider in providers {
        let Some(rules) = sheet.providers.get(provider) else {
            continue;
        };
        let Some(plan) = rules.plan.as_ref() else {
            continue;
        };
        let rate = sheet.rate(&rules.currency).map_err(|err| err.to_string())?;
        let converted =
            convert_micros(plan.monthly_price_micros, rate).map_err(|err| err.to_string())?;
        charges.insert(provider.to_owned(), converted);
    }
    Ok(charges)
}
