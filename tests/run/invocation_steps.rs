//! BDD step definitions for cache-volume mounting and for the ordering of the
//! mount, cloud-init, sync, and remote command invocations.

use mriya::test_support::CommandInvocation;
use rstest_bdd_macros::{given, then};

use super::bdd_steps::{StepError, context};
use super::test_helpers::{RunContext, RunContextResult};

/// Scripted exit status for an SSH mount command that fails outright, for
/// example because the mount point cannot be created.
const MOUNT_FAILURE_EXIT_CODE: i32 = 32;

// The mount SSH call runs before sync and the remote command, so its scripted
// response must be queued ahead of theirs.
#[given("the mount command succeeds")]
fn mount_command_succeeds(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.runner.push_success();
    Ok(Ok(run_context))
}

#[given("the mount command fails")]
fn mount_command_fails(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.runner.push_failure(MOUNT_FAILURE_EXIT_CODE);
    Ok(Ok(run_context))
}

/// Classification of a recorded runner invocation by its role in a run.
#[derive(Debug, PartialEq, Eq)]
enum InvocationRole {
    Mount,
    Rsync,
    OtherSsh,
}

fn classify_invocations(run_context: &RunContext) -> Vec<InvocationRole> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    run_context
        .runner
        .invocations()
        .iter()
        .map(|invocation| {
            if invocation.program == ssh_bin {
                let is_mount = invocation
                    .args
                    .last()
                    .is_some_and(|arg| arg.to_string_lossy().contains("mount /dev/vdb"));
                if is_mount {
                    InvocationRole::Mount
                } else {
                    InvocationRole::OtherSsh
                }
            } else {
                InvocationRole::Rsync
            }
        })
        .collect()
}

#[then("the cache volume is mounted before files are synced")]
fn mount_precedes_sync(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let roles = classify_invocations(context(run_context_result)?);
    if roles
        == [
            InvocationRole::Mount,
            InvocationRole::Rsync,
            InvocationRole::OtherSsh,
        ]
    {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected mount, rsync, remote command in that order, got {roles:?}"
        )))
    }
}

#[then("files are still synced and the remote command still runs after the failed mount")]
fn sync_and_run_follow_failed_mount(
    run_context_result: &RunContextResult,
) -> Result<(), StepError> {
    let roles = classify_invocations(context(run_context_result)?);
    // The mount ran first, so the first scripted response — the failure — was
    // consumed by it. Without this, a run that skipped the mount entirely
    // would satisfy the ordering assertion with the failure going to rsync.
    if roles.first() != Some(&InvocationRole::Mount) {
        return Err(StepError::Assertion(format!(
            "expected the failed mount to run first, got {roles:?}"
        )));
    }
    mount_precedes_sync(run_context_result)
}

/// Reports whether this SSH invocation is a cloud-init readiness poll.
fn is_cloud_init_check(invocation: &CommandInvocation) -> bool {
    invocation
        .args
        .last()
        .is_some_and(|arg| arg.to_string_lossy().contains("boot-finished"))
}

#[then("cloud-init readiness is checked before executing the remote command")]
fn cloud_init_checked_before_remote_command(
    run_context_result: &RunContextResult,
) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
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

    let cloud_init_index = ssh_invocations.iter().position(is_cloud_init_check);

    // The remote command is the final SSH call in the run, so identifying it
    // by position is exact where matching its text is not: the same payload
    // can appear inside a mount or cloud-init argument.
    let Some(last) = ssh_invocations.last() else {
        return Err(StepError::Assertion(String::from(
            "expected an ssh invocation",
        )));
    };
    let last_index = ssh_invocations.len() - 1;

    match cloud_init_index {
        Some(check_index) if check_index < last_index && !is_cloud_init_check(last) => Ok(()),
        Some(_) => Err(StepError::Assertion(String::from(
            "expected cloud-init check to run before the remote command",
        ))),
        None => Err(StepError::Assertion(String::from(
            "missing cloud-init readiness check invocation",
        ))),
    }
}
