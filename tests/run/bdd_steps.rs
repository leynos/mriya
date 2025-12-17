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

#[then("the run error mentions sync failure")]
fn sync_failure_reported(run_context: &RunContext) -> Result<(), StepError> {
    assert_failure_contains(run_context, "sync")
}

#[then("teardown failure is reported")]
fn teardown_failure_reported(run_context: &RunContext) -> Result<(), StepError> {
    assert_failure_contains(run_context, "failed to destroy instance")
}
