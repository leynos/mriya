//! BDD step definitions for the `mriya run` workflow.
//!
//! This module contains the core step definitions. Cache-related steps are
//! in the `cache_steps` submodule.

use mriya::{
    RunOrchestrator,
    sync::{RemoteCommandOutput, Syncer},
    test_support::ScriptedRunner,
};
use rstest_bdd_macros::{then, when};
use tokio::runtime::Runtime;

use super::{
    test_doubles::{ScriptedBackend, ScriptedBackendError},
    test_helpers::{
        RunContext,
        RunContextResult,
        RunFailure,
        RunFailureKind,
        RunResult,
        RunTestError,
    },
};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error(transparent)]
    Setup(#[from] RunTestError),
    #[error("assertion failed: {0}")]
    Assertion(String),
}

/// Borrows the run context from a fixture result, reporting a setup failure
/// as a step error so `then` steps fail with the original cause.
pub fn context(run_context_result: &RunContextResult) -> Result<&RunContext, StepError> {
    run_context_result
        .as_ref()
        .map_err(|err| StepError::Assertion(err.to_string()))
}

/// Classifies a run error for the scenario assertions.
fn failure_kind(err: &mriya::RunError<ScriptedBackendError>) -> RunFailureKind {
    match err {
        mriya::RunError::Provision(_) => RunFailureKind::Provision,
        mriya::RunError::Wait { .. } => RunFailureKind::Wait,
        mriya::RunError::Provisioning { .. } => RunFailureKind::Provisioning,
        mriya::RunError::ProvisioningTimeout { .. } => RunFailureKind::ProvisioningTimeout,
        mriya::RunError::Sync { .. } => RunFailureKind::Sync,
        mriya::RunError::CacheMount { .. } => RunFailureKind::CacheMount,
        mriya::RunError::Remote { .. } => RunFailureKind::Remote,
        mriya::RunError::Teardown { .. } => RunFailureKind::Teardown,
    }
}

#[when("I orchestrate a remote run for \"{command}\"")]
fn outcome(
    run_context_result: RunContextResult,
    command: String,
) -> Result<RunContextResult, StepError> {
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;
    let run_context = run_context_result?;
    let RunContext {
        backend,
        runner,
        sync_config,
        request,
        source,
        cloud_init_poll_interval_override,
        cloud_init_wait_timeout_override,
        event_log,
        source_tmp,
        ..
    } = run_context;
    let syncer = Syncer::new(sync_config.clone(), runner.clone())
        .map_err(RunTestError::from)
        .map_err(StepError::from)?;
    let mut orchestrator: RunOrchestrator<ScriptedBackend, ScriptedRunner> =
        RunOrchestrator::new(backend.clone(), syncer);
    if let Some(interval) = cloud_init_poll_interval_override {
        orchestrator = orchestrator.with_cloud_init_poll_interval(interval);
    }
    if let Some(timeout) = cloud_init_wait_timeout_override {
        orchestrator = orchestrator.with_cloud_init_wait_timeout(timeout);
    }

    let request_clone = request.clone();
    let source_clone = source.clone();
    let requested_command = command.clone();
    let result = runtime.block_on(async move {
        orchestrator
            .execute(&request_clone, &source_clone, command.as_str())
            .await
    });

    let result_enum = match result {
        Ok(output) => RunResult::Success(output),
        Err(err) => {
            let kind = failure_kind(&err);
            RunResult::Failure(RunFailure {
                kind,
                message: err.to_string(),
            })
        }
    };

    Ok(Ok(RunContext {
        backend,
        runner,
        sync_config,
        request,
        source,
        cloud_init_poll_interval_override,
        cloud_init_wait_timeout_override,
        outcome: Some(result_enum),
        event_log,
        requested_command: Some(requested_command),
        source_tmp,
    }))
}

#[then("the run result exit code is \"{code}\"")]
fn run_exit_code(run_context_result: &RunContextResult, code: i32) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
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
            "run failed unexpectedly: {}",
            err.message
        ))),
    }
}

#[then("the instance is destroyed")]
fn instance_destroyed(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
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
        RunResult::Failure(failure) if failure.message.contains(expected_substring) => Ok(()),
        other => Err(StepError::Assertion(format!(
            "unexpected outcome: {other:?}"
        ))),
    }
}

#[then("the run error is a provisioning timeout")]
fn provisioning_timeout(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    let Some(result) = &run_context.outcome else {
        return Err(StepError::Assertion(String::from("missing outcome")));
    };

    match result {
        RunResult::Failure(failure) if failure.kind == RunFailureKind::ProvisioningTimeout => {
            Ok(())
        }
        RunResult::Failure(failure) => Err(StepError::Assertion(format!(
            "expected provisioning timeout, got {kind:?}: {message}",
            kind = failure.kind,
            message = failure.message
        ))),
        RunResult::Success(_) => Err(StepError::Assertion(String::from(
            "expected failure, got success",
        ))),
    }
}

#[then("the run error includes a teardown failure note")]
fn teardown_failure_note_present(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    assert_failure_contains(run_context, "teardown also failed")
}

/// Returns the SSH payload that runs the scenario's requested command.
///
/// The unmount call follows the remote command, so the last SSH invocation is
/// not necessarily the user's command; matching the requested text keeps the
/// routing assertions pointed at the right call.
pub fn last_ssh_remote_command(run_context: &RunContext) -> Result<String, StepError> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let requested = run_context.requested_command.as_deref();
    run_context
        .runner
        .invocations()
        .into_iter()
        .rev()
        .find(|invocation| {
            invocation.program == ssh_bin
                && match (invocation.args.last(), requested) {
                    (Some(arg), Some(command)) => arg.to_string_lossy().contains(command),
                    _ => false,
                }
        })
        .and_then(|invocation| {
            invocation
                .args
                .last()
                .map(|arg| arg.to_string_lossy().into_owned())
        })
        .ok_or_else(|| {
            StepError::Assertion(String::from(
                "missing ssh invocation carrying the requested command",
            ))
        })
}

#[then("the run error kind is \"{kind}\"")]
fn run_error_kind(run_context_result: &RunContextResult, kind: String) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    let Some(RunResult::Failure(failure)) = &run_context.outcome else {
        return Err(StepError::Assertion(String::from(
            "expected a failure outcome",
        )));
    };
    let expected = match kind.as_str() {
        "cache-mount" => RunFailureKind::CacheMount,
        "provision" => RunFailureKind::Provision,
        "wait" => RunFailureKind::Wait,
        "sync" => RunFailureKind::Sync,
        "remote" => RunFailureKind::Remote,
        "teardown" => RunFailureKind::Teardown,
        other => {
            return Err(StepError::Assertion(format!(
                "unknown failure kind: {other}"
            )));
        }
    };
    if failure.kind == expected {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected {expected:?}, got {:?}: {}",
            failure.kind, failure.message
        )))
    }
}

#[then("the run error mentions sync failure")]
fn sync_failure_reported(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    assert_failure_contains(run_context, "sync")
}

#[then("the run error mentions provisioning failure")]
fn provisioning_failure_reported(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    assert_failure_contains(run_context, "provisioning")
}

#[then("teardown failure is reported")]
fn teardown_failure_reported(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    assert_failure_contains(run_context, "failed to destroy instance")
}

mod given;
