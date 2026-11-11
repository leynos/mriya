//! BDD step definitions for janitor behaviour.

use mriya::janitor::{Janitor, JanitorError};
use mriya::test_support::{json_servers, json_volumes};
use rstest_bdd_macros::{given, then, when};

use super::test_helpers::{JanitorContext, SweepOutcome, build_config};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error(transparent)]
    Setup(#[from] JanitorError),
    #[error("assertion failed: {0}")]
    Assertion(String),
}

#[given("a configured janitor for project \"{project}\" and test run \"{run_id}\"")]
fn configured_janitor(
    mut janitor_context: JanitorContext,
    project: String,
    run_id: String,
) -> Result<JanitorContext, StepError> {
    janitor_context.config = Some(build_config(project.trim(), run_id.trim())?);
    Ok(janitor_context)
}

#[cfg(test)]
mod tests {
    //! Exercises the step-level conversion that a rejected config exercises.

    use super::*;

    /// A blank project must surface as `StepError::Setup`, not a panic or an
    /// assertion failure, so the scenario reports the original cause.
    #[test]
    fn blank_project_becomes_a_setup_error() {
        let error = configured_janitor(
            JanitorContext::default(),
            String::from("   "),
            String::from("run-1"),
        )
        .expect_err("a blank project must be rejected");
        assert!(
            matches!(
                error,
                StepError::Setup(JanitorError::InvalidConfig { ref field }) if field == "project_id"
            ),
            "{error:?}"
        );
    }
}

/// Builds a janitor configuration without failing the scenario when rejected.
///
/// The rejection is the behaviour under test here, so it is recorded for the
/// `then` step instead of being converted into a step error. For example, a
/// whitespace project leaves `setup_error` set and `config` empty.
#[given("a janitor configured for project \"{project}\" and test run \"{run_id}\"")]
fn unvalidated_janitor(
    mut janitor_context: JanitorContext,
    project: String,
    run_id: String,
) -> JanitorContext {
    match build_config(project.trim(), run_id.trim()) {
        Ok(config) => janitor_context.config = Some(config),
        Err(error) => janitor_context.setup_error = Some(error),
    }
    janitor_context
}

#[then("the janitor rejects the configuration because {field} is blank")]
fn reports_invalid_config(
    janitor_context: &JanitorContext,
    field: String,
) -> Result<(), StepError> {
    match janitor_context.setup_error.as_ref() {
        Some(JanitorError::InvalidConfig { field: reported }) if reported == &field => Ok(()),
        Some(error) => Err(StepError::Assertion(format!(
            "expected a blank {field} rejection, got: {error}"
        ))),
        None => Err(StepError::Assertion(String::from(
            "expected the configuration to be rejected, but it was accepted",
        ))),
    }
}

#[given("scw lists one tagged server and one tagged volume")]
fn scw_lists_tagged_resources(
    janitor_context: JanitorContext,
) -> Result<JanitorContext, StepError> {
    let Some(config) = janitor_context.config.as_ref() else {
        return Err(StepError::Assertion(String::from(
            "test setup requires configured janitor",
        )));
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

    Ok(janitor_context)
}

#[given("scw lists a tagged server that remains after deletion")]
fn scw_lists_remaining_server(
    janitor_context: JanitorContext,
) -> Result<JanitorContext, StepError> {
    let Some(config) = janitor_context.config.as_ref() else {
        return Err(StepError::Assertion(String::from(
            "test setup requires configured janitor",
        )));
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

    Ok(janitor_context)
}

#[when("I run the janitor sweep")]
fn run_sweep(mut janitor_context: JanitorContext) -> Result<JanitorContext, StepError> {
    let Some(config) = janitor_context.config.clone() else {
        return Err(StepError::Assertion(String::from(
            "test setup requires configured janitor",
        )));
    };
    let janitor = Janitor::new(config, janitor_context.runner.clone());
    janitor_context.outcome = Some(match janitor.sweep() {
        Ok(summary) => SweepOutcome::Success(summary),
        Err(err) => SweepOutcome::Failure(err.to_string()),
    });
    Ok(janitor_context)
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
