//! BDD step definitions for the `mriya run` workflow.
//!
//! This module contains the core step definitions. Cache-related steps are
//! in the `cache_steps` submodule.

use std::time::Duration;

use mriya::{
    RunOrchestrator,
    sync::{RemoteCommandOutput, Syncer},
    test_support::ScriptedRunner,
};
use rstest_bdd_macros::{given, then, when};
use tokio::runtime::Runtime;

use super::{
    test_doubles::ScriptedBackend,
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

#[given("a ready backend and sync pipeline")]
fn ready_backend(run_context_result: RunContextResult) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    Ok(Ok(run_context))
}

#[given("the scripted runner returns exit code \"{code}\"")]
fn scripted_exit(
    run_context_result: RunContextResult,
    code: i32,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    // A configured cache creates its subdirectories in a separate SSH call
    // between the verified mount and rsync.
    if run_context.request.volume_id.is_some() {
        run_context.runner.push_success();
    }
    run_context.runner.push_success();
    run_context.runner.push_exit_code(code);
    Ok(Ok(run_context))
}

#[given("sync fails with status \"{code}\"")]
fn scripted_sync_failure(
    run_context_result: RunContextResult,
    code: i32,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.runner.push_failure(code);
    Ok(Ok(run_context))
}

#[given("a backend that fails during teardown")]
fn backend_fails_teardown(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.backend.fail_on_destroy();
    Ok(Ok(run_context))
}

#[given("a volume ID \"{volume_id}\" is configured")]
fn volume_id_configured(
    run_context_result: RunContextResult,
    volume_id: String,
) -> Result<RunContextResult, StepError> {
    let mut run_context = run_context_result?;
    run_context.request.volume_id = Some(volume_id.trim().to_owned());
    Ok(Ok(run_context))
}

#[given("cloud-init user data is configured")]
fn cloud_init_configured(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let mut run_context = run_context_result?;
    run_context.request.cloud_init_user_data = Some(String::from("cloud-init-user-data"));
    Ok(Ok(run_context))
}

#[given("the rsync step succeeds")]
fn rsync_succeeds(run_context_result: RunContextResult) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.runner.push_success();
    Ok(Ok(run_context))
}

#[given("cloud-init is already finished")]
fn cloud_init_already_finished(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.runner.push_success();
    Ok(Ok(run_context))
}

#[given("cloud-init check fails")]
fn cloud_init_check_fails(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    run_context
        .runner
        .fail_next_spawn(ssh_bin, "simulated cloud-init readiness check failure");
    Ok(Ok(run_context))
}

#[given("cloud-init provisioning times out")]
fn cloud_init_times_out(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let mut run_context = run_context_result?;
    let poll_interval = Duration::from_millis(1);
    let wait_timeout = Duration::from_millis(5);
    run_context.cloud_init_poll_interval_override = Some(poll_interval);
    run_context.cloud_init_wait_timeout_override = Some(wait_timeout);

    // Ensure enough stubbed responses are queued so the wait loop times out
    // deterministically instead of failing early due to a missing response.
    const ATTEMPT_MARGIN: u128 = 16;
    let poll_nanos = poll_interval.as_nanos().max(1);
    let min_attempts = wait_timeout.as_nanos().div_ceil(poll_nanos);
    let attempt_budget_nanos = min_attempts.saturating_add(ATTEMPT_MARGIN);
    let attempt_budget = attempt_budget_nanos.min(usize::MAX as u128) as usize;
    for _ in 0..attempt_budget {
        run_context.runner.push_exit_code(1);
    }
    Ok(Ok(run_context))
}

#[given("the remote command returns exit code \"{code}\"")]
fn remote_command_exit_code(
    run_context_result: RunContextResult,
    code: i32,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.runner.push_exit_code(code);
    Ok(Ok(run_context))
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
            let kind = match &err {
                mriya::RunError::Provision(_) => RunFailureKind::Provision,
                mriya::RunError::Wait { .. } => RunFailureKind::Wait,
                mriya::RunError::Provisioning { .. } => RunFailureKind::Provisioning,
                mriya::RunError::ProvisioningTimeout { .. } => RunFailureKind::ProvisioningTimeout,
                mriya::RunError::Sync { .. } => RunFailureKind::Sync,
                mriya::RunError::CacheMount { .. } => RunFailureKind::CacheMount,
                mriya::RunError::Remote { .. } => RunFailureKind::Remote,
                mriya::RunError::Teardown { .. } => RunFailureKind::Teardown,
            };
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
