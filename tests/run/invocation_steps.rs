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
    /// An SSH call running the scenario's requested command.
    RemoteCommand,
    /// Any other SSH call, such as a cloud-init readiness poll.
    OtherSsh,
}

/// Returns the last argument, which is the remote payload for SSH calls.
fn last_arg(invocation: &CommandInvocation) -> Option<String> {
    invocation
        .args
        .last()
        .map(|arg| arg.to_string_lossy().into_owned())
}

/// Classifies one invocation given the command the scenario requested.
///
/// Matching the payload rather than "anything that is not the mount" is what
/// makes the ordering assertions meaningful: a run that skipped the remote
/// command would otherwise still produce an `OtherSsh` tail and pass.
fn classify_invocation(
    invocation: &CommandInvocation,
    ssh_bin: &str,
    command: &str,
) -> InvocationRole {
    if invocation.program != ssh_bin {
        return InvocationRole::Rsync;
    }
    let Some(payload) = last_arg(invocation) else {
        return InvocationRole::OtherSsh;
    };
    if payload.contains("mount /dev/vdb") {
        InvocationRole::Mount
    } else if payload.contains(command) {
        InvocationRole::RemoteCommand
    } else {
        InvocationRole::OtherSsh
    }
}

/// Classifies every recorded invocation for the scenario's requested command.
///
/// Fails when the scenario never went through the `when` step, because without
/// a command there is no way to tell the remote call from a readiness poll and
/// any ordering assertion would be vacuous.
fn classify_invocations(run_context: &RunContext) -> Result<Vec<InvocationRole>, StepError> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let Some(command) = run_context.requested_command.as_deref() else {
        return Err(StepError::Assertion(String::from(
            "the scenario never requested a remote command",
        )));
    };
    Ok(run_context
        .runner
        .invocations()
        .iter()
        .map(|invocation| classify_invocation(invocation, ssh_bin, command))
        .collect())
}

#[then("the cache volume is mounted before files are synced")]
fn mount_precedes_sync(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let roles = classify_invocations(context(run_context_result)?)?;
    if roles
        == [
            InvocationRole::Mount,
            InvocationRole::Rsync,
            InvocationRole::RemoteCommand,
        ]
    {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected mount, rsync, requested remote command in that order, got {roles:?}"
        )))
    }
}

#[then("files are still synced and the remote command still runs after the failed mount")]
fn sync_and_run_follow_failed_mount(
    run_context_result: &RunContextResult,
) -> Result<(), StepError> {
    let roles = classify_invocations(context(run_context_result)?)?;
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
    last_arg(invocation).is_some_and(|payload| payload.contains("boot-finished"))
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

    // The remote command is the final SSH call in the run. Classifying it by
    // its payload, rather than by position alone, fails a run that ends on a
    // readiness poll or a mount instead of the command the scenario requested.
    let Some(last) = ssh_invocations.last() else {
        return Err(StepError::Assertion(String::from(
            "expected an ssh invocation",
        )));
    };
    let Some(command) = run_context.requested_command.as_deref() else {
        return Err(StepError::Assertion(String::from(
            "the scenario never requested a remote command",
        )));
    };
    let last_role = classify_invocation(last, ssh_bin, command);
    let last_index = ssh_invocations.len() - 1;

    match cloud_init_index {
        Some(check_index)
            if check_index < last_index && last_role == InvocationRole::RemoteCommand =>
        {
            Ok(())
        }
        Some(_) => Err(StepError::Assertion(format!(
            "expected the cloud-init check before the remote command, got {last_role:?} last"
        ))),
        None => Err(StepError::Assertion(String::from(
            "missing cloud-init readiness check invocation",
        ))),
    }
}
