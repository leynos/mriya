//! Tests for the paid campaign harness's refusal and stop policies.
//!
//! No test here reaches a provider. Each drives the harness through a scripted
//! driver, which is the point: the rules that keep a campaign from spending
//! without permission or continuing past a cleanup doubt must hold whether or
//! not a cloud is reachable.

use std::{collections::VecDeque, sync::Mutex};

use super::{
    super::{
        campaign::{
            AttemptFailure,
            CampaignError,
            JsonLinesSink,
            RecordSink,
            SweepReport,
            TeardownReport,
            require_opt_in,
            run_campaign,
        },
        record::{Record, RecordKind, RunOutcome, SCHEMA_VERSION},
        validate::validate_str,
    },
    campaign_doubles::{FailingSink, MemorySink, ScriptedDriver, plan},
    support::run_record,
};

#[test]
fn a_disabled_campaign_never_calls_the_driver() {
    let driver = ScriptedDriver::healthy(1);
    let mut sink = MemorySink::default();
    let error = run_campaign(&plan(1), None, &driver, &mut sink).expect_err("paid runs are off");
    assert!(matches!(error, CampaignError::NotEnabled { .. }), "{error}");
    assert_eq!(driver.calls(), Vec::<String>::new(), "no attempt may run");
    assert!(sink.records.is_empty(), "no record may be written");
}

#[test]
fn only_the_documented_opt_in_value_enables_a_campaign() {
    for value in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
        assert!(
            require_opt_in(value).is_err(),
            "{value:?} must not enable paid runs"
        );
    }
    assert!(require_opt_in(Some("1")).is_ok());
}

#[test]
fn a_plan_estimating_more_than_its_budget_is_refused_before_allocation() {
    // The budget is positive, so only the estimate can make the plan invalid.
    let mut plan = plan(1);
    plan.limits.max_spend_micros = 1;
    if let Some(attempt) = plan.attempts.first_mut() {
        attempt.estimated_micros = 2;
    }
    let driver = ScriptedDriver::healthy(1);
    let mut sink = MemorySink::default();
    let error =
        run_campaign(&plan, Some("1"), &driver, &mut sink).expect_err("the estimate exceeds it");
    assert!(matches!(error, CampaignError::InvalidPlan(_)), "{error}");
    assert_eq!(driver.calls(), Vec::<String>::new());
}

#[test]
fn an_attempt_longer_than_the_declared_maximum_is_refused() {
    let mut plan = plan(1);
    plan.limits.max_attempt_nanos = 1;
    let driver = ScriptedDriver::healthy(1);
    let mut sink = MemorySink::default();
    let error = run_campaign(&plan, Some("1"), &driver, &mut sink).expect_err("lifetime too long");
    assert!(matches!(error, CampaignError::InvalidPlan(_)), "{error}");
    assert_eq!(driver.calls(), Vec::<String>::new());
}

#[test]
fn a_clean_campaign_writes_a_header_and_every_run() {
    let driver = ScriptedDriver::healthy(2);
    let mut sink = MemorySink::default();
    run_campaign(&plan(2), Some("1"), &driver, &mut sink).expect("the campaign should complete");

    assert_eq!(
        sink.records.first().map(Record::kind),
        Some(RecordKind::CampaignHeader)
    );
    let runs = sink
        .records
        .iter()
        .filter(|record| record.kind() == RecordKind::Run)
        .count();
    assert_eq!(runs, 2);
    // The pre-sweep runs before any attempt.
    assert_eq!(driver.calls().first().map(String::as_str), Some("sweep"));
}

#[test]
fn a_pre_sweep_that_finds_residuals_stops_before_allocating() {
    let mut driver = ScriptedDriver::healthy(1);
    driver.sweep = Ok(SweepReport {
        residual_ids: vec![String::from("srv-left-behind")],
    });
    let mut sink = MemorySink::default();
    let error = run_campaign(&plan(1), Some("1"), &driver, &mut sink)
        .expect_err("a residual resource stops the campaign");
    assert!(
        matches!(error, CampaignError::SweepFoundResiduals(ref ids) if ids == &vec![String::from("srv-left-behind")]),
        "{error}"
    );
    assert!(
        !driver.calls().iter().any(|call| call.starts_with("run:")),
        "no attempt may run: {:?}",
        driver.calls()
    );
    let residual = sink
        .records
        .iter()
        .find(|record| record.kind() == RecordKind::ResidualResource);
    assert!(residual.is_some(), "the residual must be recorded");
}

#[test]
fn an_unverified_teardown_records_the_residual_and_stops() {
    let mut driver = ScriptedDriver::healthy(2);
    driver.teardowns = Mutex::new(VecDeque::from(vec![
        Ok(TeardownReport {
            residual_ids: vec![String::from("srv-1")],
            reason: String::from("the listing could not be read"),
        }),
        Ok(TeardownReport::verified()),
    ]));
    let mut sink = MemorySink::default();
    let error = run_campaign(&plan(2), Some("1"), &driver, &mut sink)
        .expect_err("an unverified teardown stops the campaign");
    assert!(
        matches!(error, CampaignError::CleanupDoubt { .. }),
        "{error}"
    );

    let attempts = driver
        .calls()
        .iter()
        .filter(|call| call.starts_with("run:"))
        .count();
    assert_eq!(
        attempts,
        1,
        "no second attempt may start: {:?}",
        driver.calls()
    );

    let residual = sink
        .records
        .iter()
        .find_map(|record| match record {
            Record::Residual(residual) => Some(residual.clone()),
            _ => None,
        })
        .expect("a residual record must be written");
    assert_eq!(residual.resource_ids, vec![String::from("srv-1")]);
    assert_eq!(residual.provider, "scaleway");
}

#[test]
fn a_teardown_verification_error_is_also_a_cleanup_doubt() {
    let mut driver = ScriptedDriver::healthy(1);
    driver.teardowns = Mutex::new(VecDeque::from(vec![Err(String::from(
        "the provider listing timed out",
    ))]));
    let mut sink = MemorySink::default();
    let error = run_campaign(&plan(1), Some("1"), &driver, &mut sink)
        .expect_err("an unreadable listing is a doubt, not a pass");
    assert!(
        matches!(error, CampaignError::CleanupDoubt { .. }),
        "{error}"
    );
    assert!(
        sink.records
            .iter()
            .any(|record| record.kind() == RecordKind::ResidualResource)
    );
}

#[test]
fn a_record_that_cannot_be_written_stops_the_campaign() {
    let driver = ScriptedDriver::healthy(1);
    let mut sink = FailingSink;
    let error = run_campaign(&plan(1), Some("1"), &driver, &mut sink)
        .expect_err("a campaign whose evidence cannot be written must stop");
    assert!(
        matches!(error, CampaignError::RecordWriteFailed(_)),
        "{error}"
    );
    assert!(
        driver.calls().is_empty(),
        "no allocation may precede the header write"
    );
}

#[test]
fn the_written_campaign_validates_clean() {
    let driver = ScriptedDriver::healthy(1);
    let mut sink = MemorySink::default();
    run_campaign(&plan(1), Some("1"), &driver, &mut sink).expect("the campaign should complete");

    // Re-serialize what was written and validate it as an operator would.
    let mut text = String::new();
    for record in &sink.records {
        let line = super::super::record::to_line(record).expect("the record serializes");
        text.push_str(&line);
        text.push('\n');
    }
    let campaign = validate_str(&text).expect("the harness must write a valid campaign");
    assert_eq!(campaign.runs.len(), 1);
}

#[test]
fn a_json_lines_sink_writes_one_object_per_line() {
    let mut buffer = Vec::new();
    {
        let mut sink = JsonLinesSink::new(&mut buffer);
        sink.append(&Record::Header(Box::new(plan(1).header())))
            .expect("the header writes");
    }
    let text = String::from_utf8(buffer).expect("the sink writes UTF-8");
    assert_eq!(text.lines().count(), 1);
    assert!(text.ends_with('\n'));
}

#[test]
fn an_aborted_run_is_recorded_as_a_failure_rather_than_dropped() {
    let mut record = run_record(0);
    record.outcome = RunOutcome::Aborted {
        phase: super::super::record::PhaseKind::RemoteExecution,
        summary: String::from("the workload was killed"),
    };
    let driver = ScriptedDriver {
        calls: Mutex::new(Vec::new()),
        sweep: Ok(SweepReport::default()),
        runs: Mutex::new(VecDeque::from(vec![Ok(record)])),
        teardowns: Mutex::new(VecDeque::from(vec![Ok(TeardownReport::verified())])),
    };
    let mut sink = MemorySink::default();
    run_campaign(&plan(1), Some("1"), &driver, &mut sink).expect("a failed attempt is still data");
    assert!(
        sink.records
            .iter()
            .any(|entry| matches!(entry, Record::Run(run) if run.schema == SCHEMA_VERSION))
    );
}

#[test]
fn an_attempt_that_fails_writes_a_residual_record_before_stopping() {
    // A second attempt is scripted to succeed; it must never run.
    let mut driver = ScriptedDriver::healthy(2);
    driver.runs = Mutex::new(VecDeque::from(vec![Err(AttemptFailure {
        reason: String::from("the teardown did not confirm the instance is gone"),
        resource_ids: vec![String::from("srv-1")],
    })]));
    let mut sink = MemorySink::default();
    let error = run_campaign(&plan(2), Some("1"), &driver, &mut sink)
        .expect_err("a failed attempt stops the campaign");
    assert!(matches!(error, CampaignError::AttemptFailed(_)), "{error}");
    let residual = sink
        .records
        .iter()
        .find_map(|record| match record {
            Record::Residual(residual) => Some(residual.clone()),
            _ => None,
        })
        .expect("a residual record must name the doubt");
    assert_eq!(residual.provider, "scaleway");
    assert_eq!(residual.resource_ids, vec![String::from("srv-1")]);
    assert!(residual.reason.contains("could not be verified gone"));
    assert_eq!(
        driver
            .calls()
            .iter()
            .filter(|call| call.starts_with("run:"))
            .count(),
        1,
        "no second attempt may start"
    );
}

#[test]
fn two_runs_of_one_campaign_do_not_overlap() {
    use super::super::campaign::CampaignLock;

    let lock = CampaignLock::acquire("lock-test-campaign").expect("the first lock is free");
    let second = CampaignLock::acquire("lock-test-campaign");
    assert!(
        matches!(second, Err(CampaignError::CampaignAlreadyRunning { .. })),
        "a held campaign lock must refuse a second run"
    );
    drop(lock);
    CampaignLock::acquire("lock-test-campaign").expect("the lock is released with the guard");
}
