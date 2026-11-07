//! Shared fixtures and helpers for janitor BDD scenarios.

use mriya::janitor::{DEFAULT_SCW_BIN, JanitorConfig, JanitorError, SweepSummary};
use mriya::test_support::ScriptedRunner;
use rstest::fixture;

#[derive(Clone, Debug)]
pub enum SweepOutcome {
    Success(SweepSummary),
    Failure(String),
}

#[derive(Clone, Debug)]
pub struct JanitorContext {
    pub config: Option<JanitorConfig>,
    pub runner: ScriptedRunner,
    pub outcome: Option<SweepOutcome>,
}

#[fixture]
pub fn janitor_context() -> JanitorContext {
    JanitorContext {
        config: None,
        runner: ScriptedRunner::new(),
        outcome: None,
    }
}

/// Builds a janitor configuration, reporting invalid input to the caller.
pub fn build_config(project: &str, run_id: &str) -> Result<JanitorConfig, JanitorError> {
    JanitorConfig::new(project, run_id, DEFAULT_SCW_BIN)
}
