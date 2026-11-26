//! Given steps for the `mriya run` workflow.

use std::time::Duration;

use rstest_bdd_macros::given;

use super::StepError;
use crate::run::test_helpers::RunContextResult;

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
