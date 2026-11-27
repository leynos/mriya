//! Tests for how the report groups, attributes and summarizes results.

use super::{
    super::{
        record::CacheState,
        report::{build_report, render},
    },
    support::{campaign_with, example_sheet, run_with},
};

#[test]
fn attempts_with_different_cache_classes_are_reported_separately() {
    // Cold and warm attempts on one provider are different experiments: they
    // get their own groups, so neither the per-gate cost nor the timing summary
    // averages a cache hit with a cold build.
    let cold = run_with(|run| {
        run.cache.state = CacheState::Empty;
        run.cache.mounted = true;
        run.repetition = 1;
    });
    let warm = run_with(|run| {
        run.cache.state = CacheState::Warm;
        run.cache.mounted = true;
        run.cache.size_bytes = 4096;
        run.repetition = 2;
    });
    let report = build_report(
        &campaign_with(vec![cold, warm]).expect("a valid campaign"),
        &example_sheet().expect("the example sheet parses"),
    )
    .expect("priced");
    let results = report.workloads.get("cold-build").expect("the workload");
    assert_eq!(results.len(), 2, "each cache class is its own group");
    assert!(
        report
            .capability_notes
            .iter()
            .any(|note| note.contains("empty") && note.contains("warm")),
        "the note names both classes in the protocol's spelling: {:?}",
        report.capability_notes
    );
}

#[test]
fn a_warm_up_is_costed_but_excluded_from_the_per_gate_figure() {
    let mut warm_up = run_with(|run| {
        run.role = super::super::record::RunRole::WarmUp;
        run.repetition = 1;
    });
    warm_up.lifetime.total_nanos = 3_600_000_000_000;
    warm_up.lifetime.chargeable_nanos = 3_600_000_000_000;
    let timed = run_with(|run| {
        run.repetition = 2;
        run.lifetime.total_nanos = 60_000_000_000;
        run.lifetime.chargeable_nanos = 3_600_000_000_000;
    });
    let report = build_report(
        &campaign_with(vec![warm_up, timed]).expect("a valid campaign"),
        &example_sheet().expect("the example sheet parses"),
    )
    .expect("priced");
    let results = report.workloads.get("cold-build").expect("the workload");
    let total_warm_up: u64 = results.iter().map(|r| r.warm_up_marginal_micros).sum();
    assert!(total_warm_up > 0, "the warm-up is costed");
    let timed_completed: u32 = results.iter().map(|r| r.completed).sum();
    assert_eq!(timed_completed, 1, "exactly the timed attempt is counted");
    let timed_cost: u64 = results.iter().map(|r| r.completed_marginal_micros).sum();
    assert_eq!(
        timed_cost, 8_980,
        "the per-gate cost is the timed attempt's, not the warm-up's"
    );
    assert!(
        render(&report).contains("warm-up cost"),
        "{}",
        render(&report)
    );
}

#[test]
fn the_execution_summary_uses_the_remote_execution_phase() {
    let attempt = run_with(|_| {});
    let mut campaign = campaign_with(vec![attempt]).expect("a valid campaign");
    let run_id = campaign
        .runs
        .first()
        .map(|(_, entry)| entry.run_id.clone())
        .expect("the campaign has the run just added");
    campaign.phases.push((
        3,
        super::super::record::PhaseRecord {
            schema: super::super::record::SCHEMA_VERSION,
            kind: super::super::record::RecordKind::Phase,
            campaign_id: campaign.header.campaign_id.clone(),
            run_id: run_id.clone(),
            workload_id: String::from("cold-build"),
            repetition: 1,
            phase: super::super::record::PhaseKind::RemoteExecution,
            segment: 1,
            duration_nanos: 30_000_000_000,
            status: super::super::record::PhaseStatus::Ok,
        },
    ));
    let report = build_report(
        &campaign,
        &example_sheet().expect("the example sheet parses"),
    )
    .expect("priced");
    let result = report
        .workloads
        .get("cold-build")
        .and_then(|results| results.first())
        .expect("one result");
    assert_eq!(result.minimum_execution_nanos(), Some(30_000_000_000));
}

#[test]
fn retained_storage_is_priced_into_the_campaign_total() {
    let run = run_with(|run| {
        run.retained = vec![super::super::record::RetainedStorageRecord {
            class: String::from("block-ssd"),
            bytes: 10 * 1_000_000_000,
            held_nanos: 3_600_000_000_000,
        }];
    });
    let without = build_report(
        &campaign_with(vec![run_with(|_| {})]).expect("a valid campaign"),
        &example_sheet().expect("the example sheet parses"),
    )
    .expect("priced");
    let with = build_report(
        &campaign_with(vec![run]).expect("a valid campaign"),
        &example_sheet().expect("the example sheet parses"),
    )
    .expect("priced");
    assert!(
        with.totals.storage_micros > without.totals.storage_micros,
        "retained storage must reach the campaign total"
    );
}

#[test]
fn a_charge_the_sheet_does_not_price_is_named_before_the_totals() {
    // The example sheet deliberately does not price Scaleway's public address.
    // A total that silently omitted it would be an understated bill.
    let report = build_report(
        &campaign_with(vec![run_with(|_| {})]).expect("a valid campaign"),
        &example_sheet().expect("the example sheet parses"),
    )
    .expect("priced");
    assert!(
        report
            .incomplete_totals
            .iter()
            .any(|note| note.contains("scaleway") && note.contains("IPv4")),
        "{:?}",
        report.incomplete_totals
    );
    let rendered = render(&report);
    assert!(rendered.contains("totals are incomplete"), "{rendered}");
    assert!(rendered.contains("(incomplete)"), "{rendered}");
}

#[test]
fn a_warm_up_running_a_different_command_does_not_make_timed_attempts_incomparable() {
    let warm_up = run_with(|run| {
        run.role = super::super::record::RunRole::WarmUp;
        run.command = vec![String::from("cargo"), String::from("fetch")];
        run.repetition = 1;
    });
    let timed = run_with(|run| {
        run.repetition = 2;
    });
    let report = build_report(
        &campaign_with(vec![warm_up, timed]).expect("a valid campaign"),
        &example_sheet().expect("the example sheet parses"),
    )
    .expect("priced");
    assert!(
        !report
            .comparability_problems
            .iter()
            .any(|problem| problem.contains("different commands")),
        "{:?}",
        report.comparability_problems
    );
}

#[test]
fn each_providers_subscription_is_attributed_only_to_that_provider() {
    // Boat has a plan; Scaleway does not. A group on Scaleway must not be
    // charged any of Boat's plan, and the shares must add back to the total.
    let boat = run_with(|run| {
        run.provider.provider = String::from("boat");
        run.provider.product = String::from("default");
        run.cache.state = CacheState::Absent;
        run.cache.mounted = false;
        run.workload_id = String::from("cold-build");
        run.repetition = 1;
    });
    let scaleway = run_with(|run| {
        run.provider.provider = String::from("scaleway");
        run.provider.product = String::from("DEV1-S");
        run.workload_id = String::from("warm-incremental");
        run.command = vec![String::from("cargo"), String::from("build")];
        run.repetition = 1;
    });
    // A second Boat workload: the plan must still be charged only once.
    let boat_again = run_with(|run| {
        run.provider.provider = String::from("boat");
        run.provider.product = String::from("default");
        run.cache.state = CacheState::Absent;
        run.cache.mounted = false;
        run.workload_id = String::from("warm-incremental");
        run.command = vec![String::from("cargo"), String::from("build")];
        run.repetition = 1;
    });
    let report = build_report(
        &campaign_with(vec![boat, scaleway, boat_again]).expect("a valid campaign"),
        &example_sheet().expect("the example sheet parses"),
    )
    .expect("priced");
    let boat_share: u64 = report
        .workloads
        .values()
        .flatten()
        .filter(|r| r.provider == "boat")
        .map(|r| r.subscription_micros)
        .sum();
    let scaleway_share: u64 = report
        .workloads
        .values()
        .flatten()
        .filter(|r| r.provider == "scaleway")
        .map(|r| r.subscription_micros)
        .sum();
    assert_eq!(scaleway_share, 0, "Scaleway has no plan");
    assert_eq!(
        boat_share, report.subscription_micros,
        "Boat's whole plan is attributed to Boat"
    );
}
