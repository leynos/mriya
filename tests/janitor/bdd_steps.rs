//! BDD step definitions for janitor behaviour.

use mriya::janitor::Janitor;
use rstest_bdd_macros::{given, then, when};

use super::test_helpers::{JanitorContext, SweepOutcome, build_config, json_servers, json_volumes};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("assertion failed: {0}")]
    Assertion(String),
}

#[given("a configured janitor for project \"{project}\" and test run \"{run_id}\"")]
fn configured_janitor(
    mut janitor_context: JanitorContext,
    project: String,
    run_id: String,
) -> JanitorContext {
    janitor_context.config = Some(build_config(project.trim(), run_id.trim()));
    janitor_context
}

#[given("scw lists one tagged server and one tagged volume")]
fn scw_lists_tagged_resources(janitor_context: JanitorContext) -> JanitorContext {
    let Some(config) = janitor_context.config.as_ref() else {
        panic!("test setup requires configured janitor");
    };
    let tag = config.test_run_tag();

    janitor_context.runner.push_output(
        Some(0),
        json_servers(&[
            ("srv-a", "fr-par-1", &["mriya", "ephemeral", tag.as_str()]),
            ("srv-b", "fr-par-1", &["mriya", "ephemeral"]),
        ]),
        "",
    );
    janitor_context.runner.push_success(); // delete server
    janitor_context.runner.push_output(
        Some(0),
        json_volumes(&[
            ("vol-a", "fr-par-1", &[tag.as_str()]),
            ("vol-b", "fr-par-1", &[]),
        ]),
        "",
    );
    janitor_context.runner.push_success(); // delete volume
    janitor_context
        .runner
        .push_output(Some(0), json_servers(&[("srv-b", "fr-par-1", &[])]), "");
    janitor_context
        .runner
        .push_output(Some(0), json_volumes(&[("vol-b", "fr-par-1", &[])]), "");

    janitor_context
}

#[given("scw lists a tagged server that remains after deletion")]
fn scw_lists_remaining_server(janitor_context: JanitorContext) -> JanitorContext {
    let Some(config) = janitor_context.config.as_ref() else {
        panic!("test setup requires configured janitor");
    };
    let tag = config.test_run_tag();

    janitor_context.runner.push_output(
        Some(0),
        json_servers(&[("srv-a", "fr-par-1", &[tag.as_str()])]),
        "",
    );
    janitor_context.runner.push_success(); // delete server
    janitor_context
        .runner
        .push_output(Some(0), json_volumes(&[]), "");
    // post: server still present
    janitor_context.runner.push_output(
        Some(0),
        json_servers(&[("srv-a", "fr-par-1", &[tag.as_str()])]),
        "",
    );
    janitor_context
        .runner
        .push_output(Some(0), json_volumes(&[]), "");

    janitor_context
}

#[when("I run the janitor sweep")]
fn run_sweep(mut janitor_context: JanitorContext) -> JanitorContext {
    let config = janitor_context
        .config
        .clone()
        .unwrap_or_else(|| panic!("test setup requires configured janitor"));
    let janitor = Janitor::new(config, janitor_context.runner.clone());
    janitor_context.outcome = Some(match janitor.sweep() {
        Ok(summary) => SweepOutcome::Success(summary),
        Err(err) => SweepOutcome::Failure(err.to_string()),
    });
    janitor_context
}

#[then("the janitor reports deleting {servers:u32} server and {volumes:u32} volume")]
fn reports_deletions(
    janitor_context: &JanitorContext,
    servers: u32,
    volumes: u32,
) -> Result<(), StepError> {
    let Some(outcome) = janitor_context.outcome.as_ref() else {
        return Err(StepError::Assertion(String::from("missing outcome")));
    };
    let SweepOutcome::Success(summary) = outcome else {
        return Err(StepError::Assertion(format!(
            "expected success, got: {outcome:?}"
        )));
    };
    if summary.deleted_servers == servers as usize && summary.deleted_volumes == volumes as usize {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected {servers} servers and {volumes} volumes, got {summary:?}"
        )))
    }
}

#[then("the janitor deletes servers without deleting attached volumes")]
fn deletes_servers_without_volumes(janitor_context: &JanitorContext) -> Result<(), StepError> {
    let invocations = janitor_context.runner.invocations();
    let delete_call = invocations
        .iter()
        .find(|call| {
            call.args
                .iter()
                .any(|arg| arg.to_string_lossy() == "server")
                && call
                    .args
                    .iter()
                    .any(|arg| arg.to_string_lossy() == "delete")
        })
        .ok_or_else(|| StepError::Assertion(String::from("missing server delete invocation")))?;

    let contains = |needle: &str| {
        delete_call
            .args
            .iter()
            .any(|arg| arg.to_string_lossy() == needle)
    };
    if !contains("with-volumes=none") {
        return Err(StepError::Assertion(format!(
            "expected with-volumes=none, got args: {:?}",
            delete_call.args
        )));
    }
    if !contains("--wait") {
        return Err(StepError::Assertion(String::from(
            "expected --wait flag for server delete",
        )));
    }
    Ok(())
}

#[then("the janitor reports a not-clean error")]
fn reports_not_clean(janitor_context: &JanitorContext) -> Result<(), StepError> {
    let Some(outcome) = janitor_context.outcome.as_ref() else {
        return Err(StepError::Assertion(String::from("missing outcome")));
    };
    let SweepOutcome::Failure(message) = outcome else {
        return Err(StepError::Assertion(String::from(
            "expected sweep to fail, got success",
        )));
    };
    if message.contains("resources remain after janitor sweep") {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected not-clean error, got: {message}"
        )))
    }
}
