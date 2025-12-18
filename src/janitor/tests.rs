//! Unit tests for the janitor module.

use super::*;
use crate::test_support::{ScriptedRunner, json_servers, json_volumes};
use rstest::rstest;

#[rstest]
fn janitor_config_builds_test_run_tag() {
    let cfg = JanitorConfig::new("proj", "abc", DEFAULT_SCW_BIN).expect("config should build");
    assert_eq!(cfg.test_run_tag(), "mriya-test-run-abc");
}

#[rstest]
#[case("project_id", " ", "run-1", DEFAULT_SCW_BIN)]
#[case("test_run_id", "proj", " ", DEFAULT_SCW_BIN)]
#[case("scw_bin", "proj", "run-1", "  ")]
fn janitor_config_rejects_blank_fields(
    #[case] expected_field: &str,
    #[case] project_id: &str,
    #[case] test_run_id: &str,
    #[case] scw_bin: &str,
) {
    let err =
        JanitorConfig::new(project_id, test_run_id, scw_bin).expect_err("expected invalid config");
    assert_eq!(
        err,
        JanitorError::InvalidConfig {
            field: expected_field.to_owned()
        }
    );
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
    let JanitorError::NotClean { message } = err else {
        panic!("expected NotClean, got {err:?}");
    };
    assert!(
        message.contains("srv-a@fr-par-1"),
        "expected remaining server ID, got: {message}"
    );
}

#[rstest]
fn sweep_surfaces_scw_command_failures() {
    let cfg = JanitorConfig::new("project", "run-1", DEFAULT_SCW_BIN).expect("config");
    let runner = ScriptedRunner::new();

    runner.push_output(Some(2), "", "permission denied");

    let janitor = Janitor::new(cfg, runner);
    let err = janitor.sweep().expect_err("sweep should fail");
    assert!(matches!(err, JanitorError::CommandFailure { .. }));
}

#[rstest]
#[case("malformed JSON", "not-json", None)]
#[case(
    "missing resource field",
    "{\"total_count\":0}",
    Some("missing 'servers' field")
)]
#[case(
    "item deserialisation error",
    "{\"servers\":[{\"id\":\"srv-a\"}],\"total_count\":1}",
    None
)]
#[case("unexpected JSON shape", "true", Some("unexpected JSON shape"))]
fn sweep_surfaces_parse_failures(
    #[case] scenario: &str,
    #[case] json_output: &str,
    #[case] expected_message_fragment: Option<&str>,
) {
    let cfg = JanitorConfig::new("project", "run-1", DEFAULT_SCW_BIN).expect("config");
    let runner = ScriptedRunner::new();

    runner.push_output(Some(0), json_output, "");

    let janitor = Janitor::new(cfg, runner);
    let err = janitor
        .sweep()
        .expect_err(&format!("sweep should fail for {scenario}"));

    let JanitorError::Parse { message, .. } = err else {
        panic!("expected Parse error for {scenario}, got {err:?}");
    };
    if let Some(fragment) = expected_message_fragment {
        assert!(
            message.contains(fragment),
            "expected message to contain '{fragment}' for {scenario}, got: {message}"
        );
    }
}

#[rstest]
fn sweep_surfaces_runner_failures() {
    let cfg = JanitorConfig::new("project", "run-1", DEFAULT_SCW_BIN).expect("config");
    let runner = ScriptedRunner::new();

    let janitor = Janitor::new(cfg, runner);
    let err = janitor.sweep().expect_err("sweep should fail");
    assert!(matches!(err, JanitorError::Runner(_)));
}
