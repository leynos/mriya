//! Tests for the offline equal-work report.
//!
//! The report is the last place the protocol's comparison rules can be
//! enforced before somebody publishes a number, so these tests check the
//! refusals as carefully as the arithmetic: a comparison that is not equal
//! work, or that hides a cache-capability difference, must not read as a
//! result.

use rstest::rstest;

use super::{
    super::{
        record::{CacheState, Record, RunOutcome, RunRecord, WorkloadClass, parse_line},
        report::{ReportError, build_report, render},
        validate::{Campaign, validate_str},
    },
    fixture,
    support::example_sheet,
};

/// Returns the fixture's header and run lines as a campaign.
fn example_campaign() -> Campaign {
    validate_str(&fixture("valid-campaign.jsonl")).expect("the fixture is valid")
}

/// Builds a campaign from a header and the runs given.
fn campaign_with(runs: Vec<RunRecord>) -> Campaign {
    let header_line = fixture("valid-campaign.jsonl")
        .lines()
        .next()
        .expect("the fixture has a header")
        .to_owned();
    let mut text = header_line;
    text.push('\n');
    for run in runs {
        let record = Record::Run(Box::new(run));
        text.push_str(&super::super::record::to_line(&record).expect("the run serializes"));
        text.push('\n');
    }
    validate_str(&text).expect("the constructed campaign is valid")
}

/// Returns the fixture's run record, adjusted by `change`.
fn run_with(change: impl FnOnce(&mut RunRecord)) -> RunRecord {
    let mut run = match parse_line(
        fixture("valid-campaign.jsonl")
            .lines()
            .nth(1)
            .expect("the fixture has a run line"),
    )
    .expect("the fixture's run line parses")
    {
        Record::Run(run) => *run,
        other => panic!("expected a run record, got {other:?}"),
    };
    change(&mut run);
    run
}

#[test]
fn the_fixture_is_priced_against_the_example_sheet() {
    let report = build_report(&example_campaign(), &example_sheet())
        .expect("the fixture should be priceable");
    assert_eq!(report.campaign_id, "example-2026-11-26");
    assert_eq!(report.currency, "EUR");
    let results = report
        .workloads
        .get("cold-build")
        .expect("the fixture's workload");
    assert_eq!(results.len(), 1);
    let result = results.first().expect("one result");
    assert_eq!(result.completed, 1);
    assert_eq!(result.failed, 0);
    // DEV1-S is EUR 0.00898/hour and bills a 60-minute minimum.
    assert_eq!(result.marginal_per_gate_micros(), Some(8_980));
}

#[test]
fn the_report_renders_the_cost_per_completed_gate() {
    let report = build_report(&example_campaign(), &example_sheet()).expect("priced");
    let rendered = render(&report);
    assert!(
        rendered.contains("campaign example-2026-11-26"),
        "{rendered}"
    );
    assert!(
        rendered.contains("marginal cost per completed gate EUR 0.008980"),
        "{rendered}"
    );
    assert!(rendered.contains("cache empty"), "{rendered}");
}

#[test]
fn a_failed_attempt_is_costed_but_not_counted_as_completed() {
    let run = run_with(|run| {
        run.outcome = RunOutcome::Failed {
            exit_code: 101,
            summary: String::from("compile error"),
        };
    });
    let report = build_report(&campaign_with(vec![run]), &example_sheet()).expect("priced");
    let result = report
        .workloads
        .get("cold-build")
        .and_then(|results| results.first())
        .expect("one result");
    assert_eq!(result.completed, 0);
    assert_eq!(result.failed, 1);
    assert_eq!(result.marginal_per_gate_micros(), None);
    assert!(result.failed_marginal_micros > 0);
    let rendered = render(&report);
    assert!(rendered.contains("failed-attempt cost"), "{rendered}");
    assert!(
        rendered.contains("no completed gate"),
        "a failed workload must say it has no per-gate cost: {rendered}"
    );
}

#[rstest]
#[case(CacheState::Empty, CacheState::Warm, "different cache classes")]
#[case(CacheState::Absent, CacheState::Warm, "different cache classes")]
fn a_cache_capability_difference_is_stated_rather_than_averaged(
    #[case] first: CacheState,
    #[case] second: CacheState,
    #[case] expected_note: &str,
) {
    let mut a = run_with(|run| {
        run.provider.provider = String::from("scaleway");
        run.cache.state = first;
        run.cache.mounted = first != CacheState::Absent;
        run.cache.size_bytes = u64::from(first == CacheState::Warm) * 1024;
    });
    a.provider.product = String::from("DEV1-S");
    let b = run_with(|run| {
        run.provider.provider = String::from("hetzner");
        run.provider.product = String::from("CX23");
        run.cache.state = second;
        run.cache.mounted = second != CacheState::Absent;
        run.cache.size_bytes = u64::from(second == CacheState::Warm) * 1024;
    });
    let report = build_report(&campaign_with(vec![a, b]), &example_sheet()).expect("priced");
    assert!(
        report
            .capability_notes
            .iter()
            .any(|note| note.contains(expected_note)),
        "notes: {:?}",
        report.capability_notes
    );
    assert!(render(&report).contains("capability differences"));
}

#[test]
fn a_different_command_makes_two_attempts_incomparable() {
    let a = run_with(|run| {
        run.provider.provider = String::from("scaleway");
    });
    let b = run_with(|run| {
        run.provider.provider = String::from("hetzner");
        run.provider.product = String::from("CX23");
        run.command = vec![String::from("cargo"), String::from("build")];
    });
    let report = build_report(&campaign_with(vec![a, b]), &example_sheet()).expect("priced");
    assert!(
        report
            .comparability_problems
            .iter()
            .any(|problem| problem.contains("different commands")),
        "problems: {:?}",
        report.comparability_problems
    );
}

#[test]
fn a_mixed_cpu_and_gpu_workload_is_refused_as_a_ranking() {
    let a = run_with(|run| {
        run.provider.provider = String::from("scaleway");
    });
    let b = run_with(|run| {
        run.provider.provider = String::from("hetzner");
        run.provider.product = String::from("CX23");
        run.class = WorkloadClass::Gpu;
        run.gpu = Some(super::super::record::GpuSpec {
            model: String::from("H100"),
            count: 1,
            memory_bytes_per_device: 80 * 1024 * 1024 * 1024,
            driver_version: String::from("560.0"),
            runtime_version: String::from("12.6"),
            correctness_passed: true,
        });
    });
    let report = build_report(&campaign_with(vec![a, b]), &example_sheet()).expect("priced");
    assert!(
        report
            .comparability_problems
            .iter()
            .any(|problem| problem.contains("CPU and GPU")),
        "problems: {:?}",
        report.comparability_problems
    );
}

#[test]
fn a_run_charged_less_than_its_measurement_is_refused() {
    let run = run_with(|run| {
        run.lifetime.total_nanos = 10;
        run.lifetime.chargeable_nanos = 5;
    });
    // The validator already rejects this record, so the campaign is built
    // directly: the report must refuse it too, because a reader may price a
    // file that was produced by an older or a buggy writer.
    let campaign = Campaign {
        header: example_campaign().header,
        runs: vec![(2, run)],
        phases: Vec::new(),
    };
    let error = build_report(&campaign, &example_sheet())
        .expect_err("an incoherent lifetime must not be priced");
    assert!(matches!(error, ReportError::Incoherent { .. }), "{error}");
}

#[test]
fn an_unknown_product_is_refused_rather_than_priced_at_zero() {
    let run = run_with(|run| {
        run.provider.product = String::from("NO-SUCH-PRODUCT");
    });
    let error = build_report(&campaign_with(vec![run]), &example_sheet())
        .expect_err("an unpriced product must fail");
    assert!(matches!(error, ReportError::Billing(_)), "{error}");
    assert!(error.to_string().contains("NO-SUCH-PRODUCT"), "{error}");
}

#[test]
fn boat_absorbs_an_attempt_inside_its_plan_and_reports_the_subscription() {
    let run = run_with(|run| {
        run.provider.provider = String::from("boat");
        run.provider.product = String::from("default");
        run.cache.state = CacheState::Absent;
        run.cache.mounted = false;
    });
    let report = build_report(&campaign_with(vec![run]), &example_sheet()).expect("priced");
    let result = report
        .workloads
        .get("cold-build")
        .and_then(|results| results.first())
        .expect("one result");
    // The attempt fits inside the plan's 555 included hours, so its compute is
    // free; only the start charge remains, because the example plan includes no
    // starts. The plan itself appears once in the campaign total.
    assert_eq!(result.completed, 1);
    assert_eq!(result.completed_marginal_micros, 27_600);
    assert!(result.absorbed_nanos > 0);
    assert!(report.subscription_micros > 0);
    assert!(report.totals.total_account_micros >= report.totals.marginal_micros);
    let rendered = render(&report);
    assert!(rendered.contains("of which subscription"), "{rendered}");
}

#[test]
fn a_campaign_with_a_plan_reports_both_figures() {
    let run = run_with(|run| {
        run.provider.provider = String::from("boat");
        run.provider.product = String::from("default");
        run.cache.state = CacheState::Absent;
        run.cache.mounted = false;
    });
    let report = build_report(&campaign_with(vec![run]), &example_sheet()).expect("priced");
    let rendered = render(&report);
    assert!(rendered.contains("campaign marginal cost"), "{rendered}");
    assert!(
        rendered.contains("campaign total-account cost"),
        "{rendered}"
    );
    assert!(
        rendered.contains("of which subscription"),
        "the campaign total must name the subscription it includes: {rendered}"
    );
    // The absorbed time is reported, so a reader can see why the two figures
    // differ rather than having to infer it.
    assert!(
        rendered.contains("absorbed by plan 3600.000s"),
        "the absorbed time is the chargeable lifetime the record carried: {rendered}"
    );
    assert!(
        report.totals.total_account_micros > report.totals.marginal_micros,
        "the plan must make the account figure exceed the marginal one"
    );
}

#[test]
fn a_group_whose_repetitions_used_different_caches_is_flagged() {
    // Two attempts on the same provider and product, one cold and one warm:
    // the group has a single provider, so a report that keyed the note only on
    // the first attempt's class would hide the difference.
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
    let report = build_report(&campaign_with(vec![cold, warm]), &example_sheet()).expect("priced");
    assert!(
        report
            .capability_notes
            .iter()
            .any(|note| note.contains("empty") && note.contains("warm")),
        "the note names both classes in the protocol's spelling: {:?}",
        report.capability_notes
    );
}
