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
            AttemptRecords,
            CampaignDriver,
            CampaignError,
            CampaignPlan,
            JsonLinesSink,
            PlannedAttempt,
            RecordSink,
            SweepReport,
            TeardownReport,
            require_opt_in,
            run_campaign,
        },
        record::{CampaignLimits, Record, RecordKind, RunOutcome, RunRecord, SCHEMA_VERSION},
        validate::validate_str,
    },
    support::run_record,
};

/// A driver that records every call and returns scripted results.
#[derive(Debug)]
struct ScriptedDriver {
    /// Calls made, in order, for assertions about whether one happened.
    calls: Mutex<Vec<String>>,
    /// What the pre-sweep returns.
    sweep: Result<SweepReport, String>,
    /// What each attempt returns.
    runs: Mutex<VecDeque<Result<RunRecord, String>>>,
    /// What each teardown check returns.
    teardowns: Mutex<VecDeque<Result<TeardownReport, String>>>,
}

impl ScriptedDriver {
    /// A driver whose sweep is clean and whose attempts all succeed.
    fn healthy(runs: usize) -> Self {
        let mut attempts = Vec::new();
        for index in 0..runs {
            attempts.push(Ok(run_record(index)));
        }
        Self {
            calls: Mutex::new(Vec::new()),
            sweep: Ok(SweepReport::default()),
            runs: Mutex::new(attempts.into()),
            teardowns: Mutex::new((0..runs).map(|_| Ok(TeardownReport::verified())).collect()),
        }
    }

    /// Records a call and returns the count so far.
    fn note(&self, call: String) -> usize {
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        calls.push(call);
        calls.len()
    }

    /// Returns the calls made so far.
    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl CampaignDriver for ScriptedDriver {
    fn pre_sweep(&self, _campaign_id: &str) -> Result<SweepReport, String> {
        self.note(String::from("sweep"));
        self.sweep.clone()
    }

    fn run_attempt(
        &self,
        _campaign_id: &str,
        attempt: &PlannedAttempt,
    ) -> Result<AttemptRecords, String> {
        self.note(format!("run:{}", attempt.workload_id));
        let run = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Err(String::from("no scripted run left")))?;
        Ok(AttemptRecords {
            run,
            phases: Vec::new(),
        })
    }

    fn verify_teardown(
        &self,
        _campaign_id: &str,
        _attempt: &PlannedAttempt,
        _run: &RunRecord,
    ) -> Result<TeardownReport, String> {
        self.note(String::from("verify"));
        self.teardowns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Err(String::from("no scripted teardown left")))
    }
}

/// A sink that keeps every record it was asked to write.
#[derive(Debug, Default)]
struct MemorySink {
    /// Records in the order they were appended.
    records: Vec<Record>,
}

impl RecordSink for MemorySink {
    fn append(&mut self, record: &Record) -> Result<(), String> {
        self.records.push(record.clone());
        Ok(())
    }
}

/// A sink that refuses every write, so the harness's response is observable.
#[derive(Debug, Default)]
struct FailingSink;

impl RecordSink for FailingSink {
    fn append(&mut self, _record: &Record) -> Result<(), String> { Err(String::from("disk full")) }
}

/// Builds a plan with `attempts` attempts and generous limits.
fn plan(attempts: usize) -> CampaignPlan {
    CampaignPlan {
        campaign_id: String::from("example-2026-11-26"),
        limits: CampaignLimits {
            max_spend_micros: 1_000_000,
            max_attempts: 10,
            max_attempt_nanos: 10_000_000_000,
        },
        price_sheet: None,
        providers: vec![String::from("scaleway")],
        attempts: (0..attempts)
            .map(|index| PlannedAttempt {
                workload_id: format!("cold-build-{index}"),
                provider: String::from("scaleway"),
                product: String::from("DEV1-S"),
                planned_lifetime_nanos: 1_000,
                estimated_micros: 1,
            })
            .collect(),
    }
}

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
    let mut plan = plan(1);
    plan.limits.max_spend_micros = 0;
    let driver = ScriptedDriver::healthy(1);
    let mut sink = MemorySink::default();
    let error = run_campaign(&plan, Some("1"), &driver, &mut sink).expect_err("budget is zero");
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
