//! BDD step definitions for the `mriya run` workflow.
//!
//! The steps drive the production orchestrator with a scripted backend, so
//! every scenario exercises the real step sequence, cleanup path, and error
//! composition.

use std::time::Duration;

use mriya::{
    CancelHandle,
    ResourceRef,
    RunOrchestrator,
    Signal,
    sync::Syncer,
    test_support::ScriptedRunner,
};
use rstest_bdd_macros::{given, when};
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

#[given("a backend that fails during cleanup")]
fn backend_fails_cleanup(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.backend.fail_cleanup(ResourceRef::server(
        "scripted-id",
        &run_context.request.zone,
    ));
    Ok(Ok(run_context))
}

#[given("the create request fails after allocation")]
fn create_fails_after_allocation(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.backend.fail_after_allocation();
    Ok(Ok(run_context))
}

#[given("the run is cancelled after its first allocation")]
fn cancel_after_allocation(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let mut run_context = run_context_result?;
    run_context.backend.block_create_until_cancelled();
    run_context.cancel_after_allocation = true;
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
        cancel_after_allocation,
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

    let ledger = mriya::ResourceLedger::in_memory(mriya::AllocationScope::new(
        request.zone.clone(),
        request.project_id.clone(),
    ));
    let requested_command = command.clone();
    let (cancel, cancellation) = CancelHandle::channel();
    let request_for_run = request.clone();
    let source_for_run = source.clone();
    let ledger_for_run = ledger.clone();
    let recorder = backend.clone();
    let result = runtime.block_on(async move {
        let execute = orchestrator.execute(mriya::run::RunInputs {
            request: &request_for_run,
            ledger: &ledger_for_run,
            source: &source_for_run,
            remote_command: command.as_str(),
            cancellation: &cancellation,
        });
        if !cancel_after_allocation {
            return execute.await;
        }

        // The backend records its allocation and then pends, so the observer
        // resolves first. Bounding the whole interaction means a missed
        // interrupt fails the scenario instead of hanging it.
        let limit = Duration::from_secs(30);
        let interrupt_handle = cancel.clone();
        let interrupt = async move {
            recorder.wait_until_recorded().await;
            interrupt_handle.cancel(Signal::Interrupt);
        };
        tokio::pin!(execute);
        tokio::pin!(interrupt);
        tokio::time::timeout(limit, async {
            tokio::select! {
                result = &mut execute => result,
                () = &mut interrupt => execute.await,
            }
        })
        .await
        .expect("the cancellation scenario must settle within its bound")
    });

    let result_enum = match result {
        Ok(outcome) => RunResult::Success(outcome.output),
        Err(err) => RunResult::Failure(RunFailure {
            kind: failure_kind(&err),
            message: err.to_string(),
        }),
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
        cancel_after_allocation,
        requested_command: Some(requested_command),
        source_tmp,
    }))
}

/// Classifies a run failure for scenario assertions.
fn failure_kind(
    err: &mriya::RunError<super::test_doubles::ScriptedBackendError>,
) -> RunFailureKind {
    match err {
        mriya::RunError::Provision(_) => RunFailureKind::Provision,
        mriya::RunError::Wait { .. } => RunFailureKind::Wait,
        mriya::RunError::Provisioning { .. } => RunFailureKind::Provisioning,
        mriya::RunError::ProvisioningTimeout { .. } => RunFailureKind::ProvisioningTimeout,
        mriya::RunError::Sync { .. } => RunFailureKind::Sync,
        mriya::RunError::Remote { .. } => RunFailureKind::Remote,
        // A cleanup failure that wraps a primary error reports the primary
        // kind, so a scenario still sees why the run failed even when cleanup
        // also failed. `Cleanup` is reserved for a cleanup failure after a
        // successful remote command, which has no primary error.
        mriya::RunError::Cleanup(failure) => failure
            .primary
            .as_deref()
            .map_or(RunFailureKind::Cleanup, failure_kind),
        mriya::RunError::Cancelled(_) => RunFailureKind::Cancelled,
    }
}

/// Returns the first SSH invocation's remote command string.
///
/// The first SSH call is the mount-and-mkdir step, which is what the cache
/// directory assertions need.
pub fn first_ssh_remote_command(run_context: &RunContext) -> Result<String, StepError> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let invocation = run_context
        .runner
        .invocations()
        .into_iter()
        .find(|inv| inv.program == ssh_bin)
        .ok_or_else(|| StepError::Assertion(String::from("missing ssh invocation")))?;

    let command = invocation.args.last().ok_or_else(|| {
        StepError::Assertion(String::from("ssh invocation missing remote command"))
    })?;

    Ok(command.to_string_lossy().into_owned())
}

/// Returns the last SSH payload that is not the cache-release call.
///
/// The release runs after the requested command, so asking for the last SSH
/// invocation without this filter would return `sync; ... umount ...` and hide
/// the command the scenario actually exercised.
pub fn last_ssh_remote_command(run_context: &RunContext) -> Result<String, StepError> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    run_context
        .runner
        .invocations()
        .into_iter()
        .rev()
        .filter(|invocation| invocation.program == ssh_bin)
        .filter_map(|invocation| {
            invocation
                .args
                .last()
                .map(|arg| arg.to_string_lossy().into_owned())
        })
        .find(|payload| {
            !(payload.starts_with("sync; if mountpoint -q ")
                && payload.contains("; then sudo umount "))
        })
        .ok_or_else(|| StepError::Assertion(String::from("missing ssh invocation")))
}
