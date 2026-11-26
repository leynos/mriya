//! Assertion steps for the `mriya run` workflow.
//!
//! Separated from the setup and invocation steps so each file stays focused:
//! these steps only inspect the outcome the orchestrator produced.

use mriya::sync::RemoteCommandOutput;
use rstest_bdd_macros::then;

use super::{
    bdd_steps::{StepError, context},
    test_helpers::{RunContext, RunContextResult, RunFailureKind, RunResult},
};

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

#[then("the instance is cleaned up")]
fn instance_cleaned_up(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    if run_context.backend.cleanup_calls() == 0 {
        return Err(StepError::Assertion(String::from(
            "backend.cleanup should be invoked",
        )));
    }
    // A call alone would pass for an empty ledger; the allocated server must
    // appear in the report the cleanup verified. When the scenario scripts
    // cleanup to fail, it cannot report a removal, and the failure note is
    // asserted separately.
    let removed = run_context.backend.removed_ids();
    if removed.iter().any(|id| id == "scripted-id")
        || run_context.backend.cleanup_is_scripted_to_fail()
    {
        return Ok(());
    }
    Err(StepError::Assertion(format!(
        "cleanup must report the allocated server as removed, got {removed:?}"
    )))
}

pub(super) fn assert_failure_contains(
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

#[then("the run error is a cancellation")]
fn run_cancelled(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    let Some(result) = &run_context.outcome else {
        return Err(StepError::Assertion(String::from("missing outcome")));
    };
    match result {
        RunResult::Failure(failure) if failure.kind == RunFailureKind::Cancelled => Ok(()),
        other => Err(StepError::Assertion(format!(
            "expected a cancellation, got {other:?}"
        ))),
    }
}

#[then("the run error includes a cleanup failure note")]
fn cleanup_failure_note_present(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    assert_failure_contains(run_context, "cleanup")
}

#[then("the run error names the residual resource")]
fn residual_resource_named(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    assert_failure_contains(run_context, "scripted-id")
}

#[then("the run error mentions sync failure")]
fn sync_failure_reported(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    assert_failure_contains(run_context, "sync")
}

#[then("the run error is a provisioning failure")]
fn run_error_is_provision(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    let Some(result) = &run_context.outcome else {
        return Err(StepError::Assertion(String::from("missing outcome")));
    };
    match result {
        RunResult::Failure(failure) if failure.kind == RunFailureKind::Provision => Ok(()),
        other => Err(StepError::Assertion(format!(
            "expected a provisioning failure, got {other:?}"
        ))),
    }
}

#[then("the run error mentions provisioning failure")]
fn provisioning_failure_reported(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    assert_failure_contains(run_context, "provisioning")
}
