//! BDD step definitions for the `mriya run` workflow.

use mriya::RunOrchestrator;
use mriya::sync::{RemoteCommandOutput, Syncer};
use rstest_bdd_macros::{given, then, when};
use std::time::Duration;
use tokio::runtime::Runtime;

use super::test_doubles::ScriptedBackend;
use super::test_helpers::{RunContext, RunFailure, RunFailureKind, RunResult, RunTestError};
use mriya::test_support::ScriptedRunner;

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error(transparent)]
    Setup(#[from] RunTestError),
    #[error("assertion failed: {0}")]
    Assertion(String),
}

#[given("a ready backend and sync pipeline")]
fn ready_backend(run_context: RunContext) -> RunContext {
    run_context
}

#[given("cache routing is disabled")]
fn cache_routing_disabled(mut run_context: RunContext) -> RunContext {
    run_context.sync_config.route_build_caches = false;
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

#[given("a volume ID \"{volume_id}\" is configured")]
fn volume_id_configured(mut run_context: RunContext, volume_id: String) -> RunContext {
    run_context.request.volume_id = Some(volume_id.trim().to_owned());
    // Push success for the mount command (runs via SSH before sync/run)
    run_context.runner.push_success();
    run_context
}

#[given("cloud-init user data is configured")]
fn cloud_init_configured(mut run_context: RunContext) -> RunContext {
    run_context.request.cloud_init_user_data = Some(String::from("cloud-init-user-data"));
    run_context
}

#[given("the rsync step succeeds")]
fn rsync_succeeds(run_context: RunContext) -> RunContext {
    run_context.runner.push_success();
    run_context
}

#[given("cloud-init is already finished")]
fn cloud_init_already_finished(run_context: RunContext) -> RunContext {
    run_context.runner.push_success();
    run_context
}

#[given("cloud-init check fails")]
fn cloud_init_check_fails(run_context: RunContext) -> RunContext {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    run_context
        .runner
        .fail_next_spawn(ssh_bin, "simulated cloud-init readiness check failure");
    run_context
}

#[given("cloud-init provisioning times out")]
fn cloud_init_times_out(mut run_context: RunContext) -> RunContext {
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
    run_context
}

#[given("the remote command returns exit code \"{code}\"")]
fn remote_command_exit_code(run_context: RunContext, code: i32) -> RunContext {
    run_context.runner.push_exit_code(code);
    run_context
}

#[given("the mount command fails")]
fn mount_command_fails(run_context: RunContext) -> RunContext {
    // No-op: mount uses `|| true` for graceful degradation.
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
                mriya::RunError::Remote { .. } => RunFailureKind::Remote,
                mriya::RunError::Teardown(_) => RunFailureKind::Teardown,
            };
            RunResult::Failure(RunFailure {
                kind,
                message: err.to_string(),
            })
        }
    };

    Ok(RunContext {
        backend,
        runner,
        sync_config,
        request,
        source,
        cloud_init_poll_interval_override,
        cloud_init_wait_timeout_override,
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
            "run failed unexpectedly: {}",
            err.message
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
        RunResult::Failure(failure) if failure.message.contains(expected_substring) => Ok(()),
        other => Err(StepError::Assertion(format!(
            "unexpected outcome: {other:?}"
        ))),
    }
}

#[then("the run error is a provisioning timeout")]
fn provisioning_timeout(run_context: &RunContext) -> Result<(), StepError> {
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
fn teardown_failure_note_present(run_context: &RunContext) -> Result<(), StepError> {
    assert_failure_contains(run_context, "teardown also failed")
}

fn last_ssh_remote_command(run_context: &RunContext) -> Result<String, StepError> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let invocation = run_context
        .runner
        .invocations()
        .into_iter()
        .rev()
        .find(|invocation| invocation.program == ssh_bin)
        .ok_or_else(|| StepError::Assertion(String::from("missing ssh invocation")))?;

    let command = invocation.args.last().ok_or_else(|| {
        StepError::Assertion(String::from("ssh invocation missing remote command"))
    })?;

    Ok(command.to_string_lossy().into_owned())
}

#[then("the remote command routes Cargo caches to the volume")]
fn remote_command_routes_cargo_caches(run_context: &RunContext) -> Result<(), StepError> {
    let remote_command = last_ssh_remote_command(run_context)?;
    for required in [
        "if mountpoint -q /mriya 2>/dev/null; then",
        "export CARGO_HOME=/mriya/cargo",
        "export RUSTUP_HOME=/mriya/rustup",
        "export CARGO_TARGET_DIR=/mriya/target",
        "export GOMODCACHE=/mriya/go/pkg/mod",
        "export GOCACHE=/mriya/go/build-cache",
        "export PIP_CACHE_DIR=/mriya/pip/cache",
        "export npm_config_cache=/mriya/npm/cache",
        "export YARN_CACHE_FOLDER=/mriya/yarn/cache",
        "export PNPM_STORE_PATH=/mriya/pnpm/store",
        "fi; cd",
    ] {
        if !remote_command.contains(required) {
            return Err(StepError::Assertion(format!(
                "expected remote command to include '{required}', got: {remote_command}"
            )));
        }
    }
    Ok(())
}

#[then("the remote command does not route Cargo caches")]
fn remote_command_does_not_route_cargo_caches(run_context: &RunContext) -> Result<(), StepError> {
    let remote_command = last_ssh_remote_command(run_context)?;

    const CARGO_CACHE_VARS: &[&str] = &["CARGO_TARGET_DIR=", "CARGO_HOME=", "RUSTUP_HOME="];

    if CARGO_CACHE_VARS
        .iter()
        .any(|var| remote_command.contains(var))
    {
        return Err(StepError::Assertion(format!(
            "expected remote command to avoid cache routing, got: {remote_command}"
        )));
    }
    Ok(())
}

#[then("cloud-init readiness is checked before executing the remote command")]
fn cloud_init_checked_before_remote_command(run_context: &RunContext) -> Result<(), StepError> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let ssh_invocations = run_context
        .runner
        .invocations()
        .into_iter()
        .filter(|invocation| invocation.program == ssh_bin)
        .collect::<Vec<_>>();

    if ssh_invocations.len() < 2 {
        return Err(StepError::Assertion(format!(
            "expected at least 2 ssh invocations, got {}",
            ssh_invocations.len()
        )));
    }

    let cloud_init_index = ssh_invocations.iter().position(|invocation| {
        invocation
            .args
            .last()
            .is_some_and(|arg| arg.to_string_lossy().contains("boot-finished"))
    });

    let remote_index = ssh_invocations.iter().position(|invocation| {
        invocation
            .args
            .last()
            .is_some_and(|arg| arg.to_string_lossy().contains("echo ok"))
    });

    match (cloud_init_index, remote_index) {
        (Some(ci), Some(remote)) if ci < remote => Ok(()),
        (Some(_), Some(_)) => Err(StepError::Assertion(String::from(
            "expected cloud-init check to run before the remote command",
        ))),
        (None, _) => Err(StepError::Assertion(String::from(
            "missing cloud-init readiness check invocation",
        ))),
        (_, None) => Err(StepError::Assertion(String::from(
            "missing remote command invocation",
        ))),
    }
}

#[then("the run error mentions sync failure")]
fn sync_failure_reported(run_context: &RunContext) -> Result<(), StepError> {
    assert_failure_contains(run_context, "sync")
}

#[then("the run error mentions provisioning failure")]
fn provisioning_failure_reported(run_context: &RunContext) -> Result<(), StepError> {
    assert_failure_contains(run_context, "provisioning")
}

#[then("teardown failure is reported")]
fn teardown_failure_reported(run_context: &RunContext) -> Result<(), StepError> {
    assert_failure_contains(run_context, "failed to destroy instance")
}

#[given("cache directory creation is disabled")]
fn cache_directory_creation_disabled(mut run_context: RunContext) -> RunContext {
    run_context.sync_config.create_cache_directories = false;
    run_context
}

fn first_ssh_raw_command(run_context: &RunContext) -> Result<String, StepError> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let invocation = run_context
        .runner
        .invocations()
        .into_iter()
        .find(|invocation| invocation.program == ssh_bin)
        .ok_or_else(|| StepError::Assertion(String::from("missing ssh invocation")))?;

    let command = invocation.args.last().ok_or_else(|| {
        StepError::Assertion(String::from("ssh invocation missing remote command"))
    })?;

    Ok(command.to_string_lossy().into_owned())
}

#[then("the mount command creates cache subdirectories")]
fn mount_command_creates_cache_subdirectories(run_context: &RunContext) -> Result<(), StepError> {
    // The first SSH command is the mount + mkdir cache dirs command
    let mount_command = first_ssh_raw_command(run_context)?;

    // Verify the mkdir -p command is present with cache subdirectories
    for required in ["mkdir -p", "/mriya/cargo", "/mriya/rustup", "/mriya/target"] {
        if !mount_command.contains(required) {
            return Err(StepError::Assertion(format!(
                "expected mount command to include '{required}', got: {mount_command}"
            )));
        }
    }

    // Verify the mkdir is gated by a mountpoint check
    if !mount_command.contains("if mountpoint -q /mriya") {
        return Err(StepError::Assertion(format!(
            "expected mkdir to be gated by mountpoint check, got: {mount_command}"
        )));
    }

    Ok(())
}

#[then("the mount command does not create cache subdirectories")]
fn mount_command_does_not_create_cache_subdirectories(
    run_context: &RunContext,
) -> Result<(), StepError> {
    // The first SSH command is the mount command
    let mount_command = first_ssh_raw_command(run_context)?;

    // Verify the mkdir command is NOT present for cache subdirectories
    if mount_command.contains("/mriya/cargo") || mount_command.contains("/mriya/rustup") {
        return Err(StepError::Assertion(format!(
            "expected mount command to NOT create cache directories, got: {mount_command}"
        )));
    }

    Ok(())
}
