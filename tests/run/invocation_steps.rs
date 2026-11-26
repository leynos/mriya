//! BDD step definitions for cache-volume mounting and for the ordering of the
//! mount, cloud-init, sync, remote command and unmount invocations.

use mriya::test_support::CommandInvocation;
use rstest_bdd_macros::{given, then};

use super::{
    bdd_steps::{StepError, context},
    test_helpers::{RunContext, RunContextResult},
};

/// Scripted exit status for an SSH mount command that fails outright.
const MOUNT_FAILURE_EXIT_CODE: i32 = 32;

/// A verified mount report, as the mount script prints it.
const VERIFIED_REPORT: &str = "MRIYA_CACHE_STATUS=verified DEVICE=/dev/sdb PATH=/mriya\n";

// The mount SSH call runs before sync and the remote command, so its scripted
// response must be queued ahead of theirs.
#[given("the mount command succeeds")]
fn mount_command_succeeds(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    run_context.runner.push_output(Some(0), VERIFIED_REPORT, "");
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
    CacheDirectories,
    Rsync,
    Unmount,
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
    if payload.contains("mount -o defaults") {
        InvocationRole::Mount
    } else if payload.contains("umount") {
        InvocationRole::Unmount
    } else if payload.contains("mkdir -p") && payload.contains("cargo") {
        InvocationRole::CacheDirectories
    } else if payload.contains(command) {
        InvocationRole::RemoteCommand
    } else {
        InvocationRole::OtherSsh
    }
}

/// Classifies every recorded invocation for the scenario's requested command.
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

/// Returns the position of `wanted` in `roles`.
fn position(roles: &[InvocationRole], wanted: &InvocationRole) -> Option<usize> {
    roles.iter().position(|role| role == wanted)
}

/// Reports whether any cache-directory call sits between mount and rsync.
fn directories_are_ordered(roles: &[InvocationRole], mount: usize, rsync: usize) -> bool {
    position(roles, &InvocationRole::CacheDirectories)
        .is_none_or(|directories| mount < directories && directories < rsync)
}

#[then("the cache volume is mounted before files are synced")]
fn mount_precedes_sync(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let roles = classify_invocations(context(run_context_result)?)?;
    let mount = position(&roles, &InvocationRole::Mount)
        .ok_or_else(|| StepError::Assertion(format!("missing mount invocation: {roles:?}")))?;
    let rsync = position(&roles, &InvocationRole::Rsync)
        .ok_or_else(|| StepError::Assertion(format!("missing rsync invocation: {roles:?}")))?;
    let remote = position(&roles, &InvocationRole::RemoteCommand).ok_or_else(|| {
        StepError::Assertion(format!("missing remote command invocation: {roles:?}"))
    })?;

    if !(mount < rsync && rsync < remote) {
        return Err(StepError::Assertion(format!(
            "expected mount, rsync, requested remote command in that order, got {roles:?}"
        )));
    }
    if !directories_are_ordered(&roles, mount, rsync) {
        return Err(StepError::Assertion(format!(
            "expected cache directories between mount and rsync, got {roles:?}"
        )));
    }
    Ok(())
}

#[then("the run stops before syncing when the cache cannot be verified")]
fn run_stops_before_sync(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let roles = classify_invocations(context(run_context_result)?)?;
    if roles.first() != Some(&InvocationRole::Mount) {
        return Err(StepError::Assertion(format!(
            "expected the failed mount to run first, got {roles:?}"
        )));
    }
    if roles.contains(&InvocationRole::Rsync) || roles.contains(&InvocationRole::RemoteCommand) {
        return Err(StepError::Assertion(format!(
            "a refused cache must stop before rsync and the remote command, got {roles:?}"
        )));
    }
    Ok(())
}

#[then("the cache is unmounted before teardown")]
fn unmount_after_remote_command(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let roles = classify_invocations(context(run_context_result)?)?;
    let remote = position(&roles, &InvocationRole::RemoteCommand).ok_or_else(|| {
        StepError::Assertion(format!("missing remote command invocation: {roles:?}"))
    })?;
    let unmount = position(&roles, &InvocationRole::Unmount)
        .ok_or_else(|| StepError::Assertion(format!("missing unmount invocation: {roles:?}")))?;
    if unmount > remote {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected unmount after the remote command, got {roles:?}"
        )))
    }
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

    let cloud_init_index = ssh_invocations.iter().position(is_cloud_init_check);

    let Some(command) = run_context.requested_command.as_deref() else {
        return Err(StepError::Assertion(String::from(
            "the scenario never requested a remote command",
        )));
    };
    let remote_index = ssh_invocations
        .iter()
        .position(|invocation| {
            classify_invocation(invocation, ssh_bin, command) == InvocationRole::RemoteCommand
        })
        .ok_or_else(|| StepError::Assertion(String::from("missing remote command invocation")))?;

    match cloud_init_index {
        Some(check_index) if check_index < remote_index => Ok(()),
        Some(_) => Err(StepError::Assertion(String::from(
            "expected the cloud-init check before the remote command",
        ))),
        None => Err(StepError::Assertion(String::from(
            "missing cloud-init readiness check invocation",
        ))),
    }
}

#[then("the teardown detaches the configured cache")]
fn teardown_detaches_cache(run_context_result: &RunContextResult) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    if run_context.backend.teardown_volume_id().as_deref() == Some("vol-12345") {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected teardown to detach vol-12345, got {:?}",
            run_context.backend.teardown_volume_id()
        )))
    }
}
