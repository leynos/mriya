//! Unit tests for the janitor module.

use super::*;
use crate::test_support::ScriptedRunner;
use rstest::rstest;

fn json_servers(servers: &[(&str, &str, &[&str])]) -> String {
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

fn json_volumes(volumes: &[(&str, &str, &[&str])]) -> String {
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

#[rstest]
fn janitor_config_builds_test_run_tag() {
    let cfg = JanitorConfig::new("proj", "abc", DEFAULT_SCW_BIN).expect("config should build");
    assert_eq!(cfg.test_run_tag(), "mriya-test-run-abc");
}

#[rstest]
fn sweep_deletes_only_tagged_resources() {
    let cfg = JanitorConfig::new("project", "run-1", DEFAULT_SCW_BIN).expect("config");
    let runner = ScriptedRunner::new();

    // list servers (pre)
    runner.push_output(
        Some(0),
        json_servers(&[
            (
                "srv-a",
                "fr-par-1",
                &["mriya", "ephemeral", "mriya-test-run-run-1"],
            ),
            ("srv-b", "fr-par-1", &["mriya", "ephemeral"]),
        ]),
        "",
    );
    // delete server srv-a
    runner.push_success();
    // list volumes (pre)
    runner.push_output(
        Some(0),
        json_volumes(&[
            ("vol-a", "fr-par-1", &["mriya-test-run-run-1"]),
            ("vol-b", "fr-par-1", &[]),
        ]),
        "",
    );
    // delete volume vol-a
    runner.push_success();
    // list servers (post)
    runner.push_output(
        Some(0),
        json_servers(&[("srv-b", "fr-par-1", &["mriya", "ephemeral"])]),
        "",
    );
    // list volumes (post)
    runner.push_output(Some(0), json_volumes(&[("vol-b", "fr-par-1", &[])]), "");

    let janitor = Janitor::new(cfg, runner.clone());
    let summary = janitor.sweep().expect("sweep should succeed");
    assert_eq!(
        summary,
        SweepSummary {
            deleted_servers: 1,
            deleted_volumes: 1
        }
    );

    let invocations = runner.invocations();
    let delete_calls = invocations
        .iter()
        .filter(|call| {
            call.args
                .iter()
                .any(|arg| arg.to_string_lossy() == "delete")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        delete_calls.len(),
        2,
        "expected one server + one volume delete"
    );
}

#[rstest]
fn sweep_errors_when_tagged_resources_remain() {
    let cfg = JanitorConfig::new("project", "run-1", DEFAULT_SCW_BIN).expect("config");
    let runner = ScriptedRunner::new();

    // list servers (pre): has one tagged server
    runner.push_output(
        Some(0),
        json_servers(&[(
            "srv-a",
            "fr-par-1",
            &["mriya", "ephemeral", "mriya-test-run-run-1"],
        )]),
        "",
    );
    // delete server srv-a fails to remove (but command succeeds)
    runner.push_success();
    // list volumes (pre)
    runner.push_output(Some(0), json_volumes(&[]), "");
    // list servers (post): still present
    runner.push_output(
        Some(0),
        json_servers(&[(
            "srv-a",
            "fr-par-1",
            &["mriya", "ephemeral", "mriya-test-run-run-1"],
        )]),
        "",
    );
    // list volumes (post)
    runner.push_output(Some(0), json_volumes(&[]), "");

    let janitor = Janitor::new(cfg, runner);
    let err = janitor.sweep().expect_err("sweep should fail");
    assert!(matches!(err, JanitorError::NotClean { .. }));
}
