//! Doubles for the campaign tests: a scripted driver, a memory sink, and a
//! sink that refuses every write.

use std::{collections::VecDeque, sync::Mutex};

use super::{
    super::{
        campaign::{
            AttemptRecords,
            CampaignDriver,
            CampaignPlan,
            PlannedAttempt,
            RecordSink,
            SweepReport,
            TeardownReport,
        },
        record::{CampaignLimits, Record, RunRecord},
    },
    support::run_record,
};

/// A driver that records every call and returns scripted results.
#[derive(Debug)]
pub struct ScriptedDriver {
    /// Calls made, in order, for assertions about whether one happened.
    pub calls: Mutex<Vec<String>>,
    /// What the pre-sweep returns.
    pub sweep: Result<SweepReport, String>,
    /// What each attempt returns.
    pub runs: Mutex<VecDeque<Result<RunRecord, String>>>,
    /// What each teardown check returns.
    pub teardowns: Mutex<VecDeque<Result<TeardownReport, String>>>,
}

impl ScriptedDriver {
    /// A driver whose sweep is clean and whose attempts all succeed.
    pub fn healthy(runs: usize) -> Self {
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
    pub fn calls(&self) -> Vec<String> {
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
            resource_ids: Vec::new(),
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
pub struct MemorySink {
    /// Records in the order they were appended.
    pub records: Vec<Record>,
}

impl RecordSink for MemorySink {
    fn append(&mut self, record: &Record) -> Result<(), String> {
        self.records.push(record.clone());
        Ok(())
    }
}

/// A sink that refuses every write, so the harness's response is observable.
#[derive(Debug, Default)]
pub struct FailingSink;

impl RecordSink for FailingSink {
    fn append(&mut self, _record: &Record) -> Result<(), String> { Err(String::from("disk full")) }
}

/// Builds a plan with `attempts` attempts and generous limits.
pub fn plan(attempts: usize) -> CampaignPlan {
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
                repetition: u32::try_from(index).unwrap_or(0).saturating_add(1),
                role: super::super::record::RunRole::Timed,
                provider: String::from("scaleway"),
                product: String::from("DEV1-S"),
                planned_lifetime_nanos: 1_000,
                estimated_micros: 1,
            })
            .collect(),
    }
}
