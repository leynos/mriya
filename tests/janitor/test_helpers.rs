//! Shared fixtures and helpers for janitor BDD scenarios.

use mriya::janitor::{DEFAULT_SCW_BIN, JanitorConfig, SweepSummary};
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

pub fn json_servers(servers: &[(&str, &str, &[&str])]) -> String {
    let items = servers
        .iter()
        .map(|(id, zone, tags)| {
            let tags_json = tags
                .iter()
                .map(|tag| format!("\"{tag}\""))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{\"id\":\"{id}\",\"zone\":\"{zone}\",\"tags\":[{tags_json}]}}")
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

pub fn json_volumes(volumes: &[(&str, &str, &[&str])]) -> String {
    let items = volumes
        .iter()
        .map(|(id, zone, tags)| {
            let tags_json = tags
                .iter()
                .map(|tag| format!("\"{tag}\""))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{\"id\":\"{id}\",\"zone\":\"{zone}\",\"tags\":[{tags_json}]}}")
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

pub fn build_config(project: &str, run_id: &str) -> JanitorConfig {
    JanitorConfig::new(project, run_id, DEFAULT_SCW_BIN)
        .unwrap_or_else(|err| panic!("janitor config should be valid: {err}"))
}
