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
pub(super) enum InvocationRole {
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
/// Reports whether this invocation runs SSH, directly or through `timeout`.
fn is_ssh_invocation(invocation: &CommandInvocation, ssh_bin: &str) -> bool {
    invocation.program == ssh_bin
        || (invocation.program == "timeout" && invocation.args.iter().any(|arg| arg == ssh_bin))
}

pub(super) fn classify_invocation(
    invocation: &CommandInvocation,
    ssh_bin: &str,
    command: &str,
) -> InvocationRole {
    if !is_ssh_invocation(invocation, ssh_bin) {
        return InvocationRole::Rsync;
    }
    let Some(payload) = last_arg(invocation) else {
        return InvocationRole::OtherSsh;
    };
    // Generated scripts end with their own status line, never with the user's
    // command, so an exact suffix match identifies the command whatever words
    // it contains.
    if payload.trim_end().ends_with(command.trim_end()) {
        return InvocationRole::RemoteCommand;
    }
    if payload.contains("mount -o defaults") {
        InvocationRole::Mount
    } else if payload.contains("MRIYA_CACHE_STATUS=unmounted") {
        InvocationRole::Unmount
    } else if is_cache_directory_payload(&payload) {
        InvocationRole::CacheDirectories
    } else {
        InvocationRole::OtherSsh
    }
}

/// Reports whether `payload` is the automatic cache-directory command.
///
/// The production command names cache subdirectories that no ordinary build
/// command uses; a requested command such as `mkdir -p /mriya/cargo` is
/// matched as the remote command first and never reaches this check.
fn is_cache_directory_payload(payload: &str) -> bool {
    payload.contains("sudo mkdir -p")
        && (payload.contains("/rustup") || payload.contains("/pnpm/store"))
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

/// Reports whether every cache-directory call sits between mount and rsync.
fn directories_are_ordered(roles: &[InvocationRole], mount: usize, rsync: usize) -> bool {
    roles
        .iter()
        .enumerate()
        .filter(|(_, role)| **role == InvocationRole::CacheDirectories)
        .all(|(index, _)| mount < index && index < rsync)
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
    let run_context = context(run_context_result)?;
    let events = run_context
        .event_log
        .lock()
        .map_err(|error| StepError::Assertion(format!("event log poisoned: {error}")))?;
    let teardown = events
        .iter()
        .position(|event| event == "teardown")
        .ok_or_else(|| StepError::Assertion(format!("missing teardown event: {events:?}")))?;
    let unmount_event = events
        .iter()
        .position(|event| event == "unmount")
        .ok_or_else(|| StepError::Assertion(format!("missing unmount event: {events:?}")))?;
    // A failed unmount reports its own status, so a recorded call alone is not
    // evidence that the cache was flushed. Match the invocation by role and
    // check the output it produced.
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let requested = run_context.requested_command.as_deref().unwrap_or_default();
    let unmount_succeeded = run_context
        .runner
        .invocations()
        .iter()
        .find(|invocation| {
            classify_invocation(invocation, ssh_bin, requested) == InvocationRole::Unmount
        })
        .is_some_and(|invocation| {
            invocation.exit_code == Some(0)
                && invocation.stdout.contains("MRIYA_CACHE_STATUS=unmounted")
        });
    let ordered = unmount > remote && unmount_event < teardown;
    if ordered && unmount_succeeded {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected remote command, unmount, teardown order; roles {roles:?}, events {events:?}"
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
        .filter(|invocation| is_ssh_invocation(invocation, ssh_bin))
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
    let expected = run_context.request.volume_id.as_deref().ok_or_else(|| {
        StepError::Assertion(String::from("the scenario never configured a cache volume"))
    })?;
    let actual = run_context.backend.teardown_volume_id();
    if actual.as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected teardown to detach {expected}, got {actual:?}"
        )))
    }
}
