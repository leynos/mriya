//! Tests for reading a campaign file and reporting its defects.

use rstest::rstest;

use super::{
    super::{
        record::{
            CampaignHeader,
            CampaignLimits,
            Record,
            RecordKind,
            RunRecord,
            SCHEMA_VERSION,
            parse_line,
            to_line,
        },
        validate::{Complaint, format_complaints, validate_str},
    },
    fixture,
};

/// Returns the complaint messages as one string, so a case can assert on the
/// presence of a specific complaint without depending on list ordering.
fn messages(complaints: &[Complaint]) -> String { format_complaints(complaints) }

#[test]
fn an_empty_file_reports_both_missing_header_and_missing_runs() {
    let complaints = validate_str("").expect_err("an empty file is not a campaign");
    let rendered = messages(&complaints);
    assert!(rendered.contains("no campaign header record"), "{rendered}");
    assert!(rendered.contains("no run records"), "{rendered}");
}

#[test]
fn a_valid_fixture_reports_no_complaint() {
    let campaign = validate_str(&fixture("valid-campaign.jsonl"))
        .expect("the valid fixture should validate clean");
    assert_eq!(campaign.id(), "example-2026-11-26");
    assert_eq!(campaign.runs.len(), 1);
    assert_eq!(campaign.phases.len(), 7);
}

#[test]
fn a_header_with_no_runs_is_reported() {
    let complaints =
        validate_str(&fixture("header-only.jsonl")).expect_err("a header alone proves nothing");
    let rendered = messages(&complaints);
    assert!(rendered.contains("no run records"), "{rendered}");
    assert!(!rendered.contains("no campaign header"), "{rendered}");
}

#[rstest]
#[case("campaign header repeated", "two headers are ambiguous")]
#[case(
    "schema 2 is not readable",
    "an unknown schema is refused, not guessed"
)]
#[case("missing pin workload_id", "an empty workload identifier is named")]
#[case("missing pin tree.commit", "an empty commit is named")]
#[case(
    "missing pin allocation.cpu_count",
    "a zero CPU count is a missing pin"
)]
#[case("repetition numbers start at one", "repetition zero is rejected")]
#[case(
    "a GPU experiment must record its accelerator details",
    "a GPU class without accelerator details is incoherent"
)]
#[case(
    "campaign stopped with unverified resources",
    "a residual record is a cleanup doubt"
)]
#[case("missing pin toolchain.compiler", "an empty compiler name is named")]
#[case("malformed record", "a non-JSON line is reported")]
fn the_defective_fixture_reports_each_named_defect(#[case] needle: &str, #[case] why: &str) {
    let complaints =
        validate_str(&fixture("defective-campaign.jsonl")).expect_err("the fixture is defective");
    let rendered = messages(&complaints);
    assert!(
        rendered.contains(needle),
        "{why}: expected {needle:?} in\n{rendered}"
    );
}

#[test]
fn a_file_whose_every_run_line_is_unreadable_has_no_run_records() {
    let text = format!(
        "{}\nnot a record\n",
        fixture("header-only.jsonl").trim_end()
    );
    let complaints = validate_str(&text).expect_err("an unreadable run is not a run");
    let rendered = messages(&complaints);
    assert!(rendered.contains("malformed record"), "{rendered}");
    assert!(rendered.contains("no run records"), "{rendered}");
}

#[test]
fn every_defect_is_reported_rather_than_only_the_first() {
    let complaints =
        validate_str(&fixture("defective-campaign.jsonl")).expect_err("the fixture is defective");
    // The fixture carries at least a dozen independent problems; a validator
    // that stopped at the first would report one.
    assert!(
        complaints.len() >= 10,
        "expected the whole defect list, got {}: {}",
        complaints.len(),
        messages(&complaints)
    );
}

#[test]
fn a_record_round_trips_through_a_line() {
    let header = CampaignHeader {
        schema: SCHEMA_VERSION,
        kind: RecordKind::CampaignHeader,
        campaign_id: String::from("round-trip"),
        limits: Some(CampaignLimits {
            max_spend_micros: 1,
            max_attempts: 2,
            max_attempt_nanos: 3,
        }),
        price_sheet: None,
        providers: vec![String::from("scaleway")],
    };
    let record = Record::Header(Box::new(header.clone()));
    let line = to_line(&record).expect("the header should serialize");
    let parsed = parse_line(&line).expect("the line should parse");
    assert_eq!(parsed, record);
    assert_eq!(parsed.kind(), RecordKind::CampaignHeader);
}

#[test]
fn an_unknown_kind_is_named_rather_than_treated_as_malformed_json() {
    let err = parse_line(r#"{"schema":1,"kind":"no-such-kind"}"#).expect_err("unknown kinds fail");
    assert!(
        err.to_string()
            .contains("unknown record kind: no-such-kind"),
        "{err}"
    );
}

#[test]
fn a_line_without_a_kind_is_reported_as_missing_one() {
    let err = parse_line(r#"{"schema":1}"#).expect_err("a kindless line fails");
    assert!(err.to_string().contains("no kind field"), "{err}");
}

#[test]
fn a_run_charged_less_than_its_measured_lifetime_is_incoherent() {
    let mut run = valid_run();
    run.lifetime.total_nanos = 20;
    run.lifetime.chargeable_nanos = 10;
    let text = campaign_with_run(&run);
    let complaints = validate_str(&text).expect_err("the lifetime contradicts itself");
    assert!(
        messages(&complaints).contains("shorter than the measured lifetime"),
        "{}",
        messages(&complaints)
    );
}

#[test]
fn a_run_charged_more_than_its_measured_lifetime_is_accepted() {
    // A provider's granularity or minimum legitimately rounds a short attempt
    // up, so a longer chargeable lifetime is the normal case rather than a
    // defect.
    let mut run = valid_run();
    run.lifetime.total_nanos = 120_000_000_000;
    run.lifetime.chargeable_nanos = 3_600_000_000_000;
    let campaign =
        validate_str(&campaign_with_run(&run)).expect("rounding up to a whole hour is normal");
    assert_eq!(campaign.runs.len(), 1);
}

#[test]
fn a_run_with_no_measured_lifetime_is_incoherent() {
    let mut run = valid_run();
    run.lifetime.total_nanos = 0;
    run.lifetime.chargeable_nanos = 0;
    let complaints = validate_str(&campaign_with_run(&run)).expect_err("a zero lifetime is wrong");
    assert!(
        messages(&complaints).contains("non-zero lifetime"),
        "{}",
        messages(&complaints)
    );
}

#[test]
fn a_cpu_run_carrying_accelerator_details_is_incoherent() {
    let mut run = valid_run();
    run.gpu = Some(super::super::record::GpuSpec {
        model: String::from("H100"),
        count: 1,
        memory_bytes_per_device: 1,
        driver_version: String::from("560.0"),
        runtime_version: String::from("12.6"),
        correctness_passed: true,
    });
    let complaints =
        validate_str(&campaign_with_run(&run)).expect_err("CPU runs carry no GPU details");
    assert!(
        messages(&complaints).contains("must not carry accelerator details"),
        "{}",
        messages(&complaints)
    );
}

#[test]
fn a_run_claiming_a_cache_it_did_not_mount_is_incoherent() {
    let mut run = valid_run();
    run.cache.mounted = false;
    let complaints = validate_str(&campaign_with_run(&run)).expect_err("the cache claim is false");
    assert!(
        messages(&complaints).contains("did not have it mounted"),
        "{}",
        messages(&complaints)
    );
}

#[test]
fn a_gpu_run_without_a_passing_correctness_check_is_a_failure() {
    let mut run = valid_run();
    run.class = super::super::record::WorkloadClass::Gpu;
    run.gpu = Some(super::super::record::GpuSpec {
        model: String::from("H100"),
        count: 1,
        memory_bytes_per_device: 1,
        driver_version: String::from("560.0"),
        runtime_version: String::from("12.6"),
        correctness_passed: false,
    });
    let complaints =
        validate_str(&campaign_with_run(&run)).expect_err("an unverified GPU run cannot rank");
    assert!(
        messages(&complaints).contains("without a passing correctness check"),
        "{}",
        messages(&complaints)
    );
}

/// Returns a run record that validates clean on its own.
fn valid_run() -> RunRecord {
    match parse_line(
        fixture("valid-campaign.jsonl")
            .lines()
            .nth(1)
            .expect("the fixture has a run line"),
    )
    .expect("the fixture's run line parses")
    {
        Record::Run(run) => *run,
        other => panic!("expected a run record, got {other:?}"),
    }
}

/// Wraps `run` in a campaign whose header matches it.
fn campaign_with_run(run: &RunRecord) -> String {
    let header = CampaignHeader {
        schema: SCHEMA_VERSION,
        kind: RecordKind::CampaignHeader,
        campaign_id: run.campaign_id.clone(),
        limits: None,
        price_sheet: None,
        providers: vec![run.provider.provider.clone()],
    };
    let header_line = to_line(&Record::Header(Box::new(header))).expect("header serializes");
    let run_line = to_line(&Record::Run(Box::new(run.clone()))).expect("run serializes");
    format!("{header_line}\n{run_line}\n")
}
