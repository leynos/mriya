//! Shared fixtures and helpers for janitor BDD scenarios.

use mriya::janitor::{DEFAULT_SCW_BIN, JanitorConfig, JanitorError, SweepSummary};
use mriya::test_support::ScriptedRunner;
use rstest::fixture;

#[derive(Clone, Debug)]
pub enum SweepOutcome {
    Success(SweepSummary),
    Failure(String),
}

#[derive(Clone, Debug, Default)]
pub struct JanitorContext {
    pub config: Option<JanitorConfig>,
    pub runner: ScriptedRunner,
    pub outcome: Option<SweepOutcome>,
    /// Set when a scenario deliberately supplies configuration the builder
    /// should reject, so the rejection itself is the observed behaviour.
    pub setup_error: Option<JanitorError>,
}

#[fixture]
pub fn janitor_context() -> JanitorContext {
    JanitorContext {
        config: None,
        runner: ScriptedRunner::new(),
        outcome: None,
        setup_error: None,
    }
}

/// Builds a janitor configuration, reporting invalid input to the caller.
///
/// # Example
///
/// A non-blank pair yields a usable config, while whitespace alone yields
/// [`JanitorError::InvalidConfig`] naming the offending field instead of a
/// config that would sweep every project.
///
/// ```ignore
/// let config = build_config("project", "run-1")?;
/// assert_eq!(config.test_run_tag(), "mriya-test-run-run-1");
/// assert!(matches!(
///     build_config("   ", "run-1"),
///     Err(JanitorError::InvalidConfig { ref field }) if field == "project_id"
/// ));
/// ```
pub fn build_config(project: &str, run_id: &str) -> Result<JanitorConfig, JanitorError> {
    JanitorConfig::new(project, run_id, DEFAULT_SCW_BIN)
}
