//! BDD step definitions for the `mriya run` workflow.

use mriya::sync::{RemoteCommandOutput, Syncer};
use mriya::{RunError, RunOrchestrator};
use rstest_bdd_macros::{given, then, when};
use tokio::runtime::Runtime;

use mriya::test_support::ScriptedRunner;
use super::test_doubles::{ScriptedBackend, ScriptedBackendError};
use super::test_helpers::{RunContext, RunOutcome, RunTestError};

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
fn orchestrate_run(run_context: RunContext, command: String) -> Result<RunOutcome, StepError> {
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;
    let backend = run_context.backend.clone();
    let syncer = Syncer::new(run_context.sync_config, run_context.runner)
        .map_err(RunTestError::from)
        .map_err(StepError::from)?;
    let request = run_context.request;
    let source = run_context.source;
    let orchestrator: RunOrchestrator<ScriptedBackend, ScriptedRunner> = RunOrchestrator::new(
        backend.clone(),
        syncer,
    );

    let remote_command = command;
    let result = runtime.block_on(async move {
        orchestrator
            .execute(&request, &source, remote_command.as_str())
            .await
    });

    Ok(RunOutcome {
        backend,
        result: std::sync::Arc::new(result),
    })
}

#[then("the run result exit code is \"{code}\"")]
fn run_exit_code(outcome: &RunOutcome, code: i32) -> Result<(), StepError> {
    match outcome.result.as_ref() {
        Ok(RemoteCommandOutput {
            exit_code: Some(actual),
            ..
        }) if *actual == code => Ok(()),
        Ok(other) => Err(StepError::Assertion(format!(
            "expected exit code {code}, got {:?}",
            other.exit_code
        ))),
        Err(err) => Err(StepError::Assertion(format!(
            "run failed unexpectedly: {err}"
        ))),
    }
}

#[then("the instance is destroyed")]
fn instance_destroyed(outcome: &RunOutcome) -> Result<(), StepError> {
    if outcome.backend.destroy_calls() > 0 {
        Ok(())
    } else {
        Err(StepError::Assertion(String::from(
            "backend.destroy should be invoked",
        )))
    }
}

#[then("the run error mentions sync failure")]
fn sync_failure_reported(outcome: &RunOutcome) -> Result<(), StepError> {
    match outcome.result.as_ref() {
        Err(RunError::Sync { .. }) => Ok(()),
        other => Err(StepError::Assertion(format!(
            "unexpected outcome: {other:?}"
        ))),
    }
}

#[then("teardown failure is reported")]
fn teardown_failure_reported(outcome: &RunOutcome) -> Result<(), StepError> {
    match outcome.result.as_ref() {
        Err(RunError::Teardown(ScriptedBackendError::Destroy)) => Ok(()),
        other => Err(StepError::Assertion(format!(
            "unexpected outcome: {other:?}"
        ))),
    }
}
