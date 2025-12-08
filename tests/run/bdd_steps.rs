//! BDD step definitions for the `mriya run` workflow.

use mriya::RunOrchestrator;
use mriya::sync::{RemoteCommandOutput, Syncer};
use rstest_bdd_macros::{given, then, when};
use tokio::runtime::Runtime;

use super::test_doubles::ScriptedBackend;
use super::test_helpers::{RunContext, RunResult, RunTestError};
use mriya::test_support::ScriptedRunner;

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error(transparent)]
    Setup(#[from] RunTestError),
    #[error("assertion failed: {0}")]
    Assertion(String),
}

#[given("a ready backend and sync pipeline")]
const fn ready_backend(run_context: RunContext) -> RunContext {
    run_context
}

#[given("the scripted runner returns exit code \"{code}\"")]
fn scripted_exit(run_context: RunContext, code: i32) -> RunContext {
    run_context.runner.push_success();
    run_context.runner.push_exit_code(code);
    run_context
}

#[given("sync fails with status \"{code}\"")]
fn scripted_sync_failure(run_context: RunContext, code: i32) -> RunContext {
    run_context.runner.push_failure(code);
    run_context
}

#[given("a backend that fails during teardown")]
fn backend_fails_teardown(run_context: RunContext) -> RunContext {
    run_context.backend.fail_on_destroy();
    run_context
}

#[when("I orchestrate a remote run for \"{command}\"")]
fn outcome(run_context: RunContext, command: String) -> Result<RunContext, StepError> {
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;
    let RunContext {
        backend,
        runner,
        sync_config,
        request,
        source,
        source_tmp,
        ..
    } = run_context;
    let syncer = Syncer::new(sync_config.clone(), runner.clone())
        .map_err(RunTestError::from)
        .map_err(StepError::from)?;
    let orchestrator: RunOrchestrator<ScriptedBackend, ScriptedRunner> =
        RunOrchestrator::new(backend.clone(), syncer);

    let request_clone = request.clone();
    let source_clone = source.clone();
    let result = runtime.block_on(async move {
        orchestrator
            .execute(&request_clone, &source_clone, command.as_str())
            .await
    });

    let result_enum = match result {
        Ok(output) => RunResult::Success(output),
        Err(err) => RunResult::Failure(err.to_string()),
    };

    Ok(RunContext {
        backend,
        runner,
        sync_config,
        request,
        source,
        outcome: Some(result_enum),
        source_tmp,
    })
}

#[then("the run result exit code is \"{code}\"")]
fn run_exit_code(run_context: &RunContext, code: i32) -> Result<(), StepError> {
    let Some(result) = &run_context.outcome else {
        return Err(StepError::Assertion(String::from("missing outcome")));
    };

    match result {
        RunResult::Success(RemoteCommandOutput {
            exit_code: Some(actual),
            ..
        }) if *actual == code => Ok(()),
        RunResult::Success(other) => Err(StepError::Assertion(format!(
            "expected exit code {code}, got {:?}",
            other.exit_code
        ))),
        RunResult::Failure(err) => Err(StepError::Assertion(format!(
            "run failed unexpectedly: {err}"
        ))),
    }
}

#[then("the instance is destroyed")]
fn instance_destroyed(run_context: &RunContext) -> Result<(), StepError> {
    if run_context.backend.destroy_calls() > 0 {
        Ok(())
    } else {
        Err(StepError::Assertion(String::from(
            "backend.destroy should be invoked",
        )))
    }
}

fn assert_failure_contains(
    run_context: &RunContext,
    expected_substring: &str,
) -> Result<(), StepError> {
    let Some(result) = &run_context.outcome else {
        return Err(StepError::Assertion(String::from("missing outcome")));
    };

    match result {
        RunResult::Failure(message) if message.contains(expected_substring) => Ok(()),
        other => Err(StepError::Assertion(format!(
            "unexpected outcome: {other:?}"
        ))),
    }
}

#[then("the run error mentions sync failure")]
fn sync_failure_reported(run_context: &RunContext) -> Result<(), StepError> {
    assert_failure_contains(run_context, "sync")
}

#[then("teardown failure is reported")]
fn teardown_failure_reported(run_context: &RunContext) -> Result<(), StepError> {
    assert_failure_contains(run_context, "failed to destroy instance")
}
