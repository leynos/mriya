//! Command execution helpers shared by the runner implementations.
//!
//! These helpers own the mechanics of waiting for a child, bounding it, and
//! draining its output while a termination is escalated.

use std::{
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::{
    CommandOutput,
    SyncError,
    registry::{
        Registration,
        escalate_registered_child,
        poll_registered_child,
        register_child,
        terminate_registered_child,
    },
    types::{forward_stream, join_forwarder},
};

/// Joins a forwarder, escalating a stalled termination while it drains.
///
/// The launcher is already reaped here; a descendant that ignored `SIGTERM`
/// can still hold the pipe, and the escalation deadline only elapses if this
/// loop keeps checking it.
pub(super) fn join_forwarder_escalating(
    handle: Option<thread::JoinHandle<Result<String, SyncError>>>,
    program: &str,
    stream_name: &str,
    id: u64,
) -> Result<String, SyncError> {
    let Some(forwarder) = handle else {
        return Ok(String::new());
    };
    while !forwarder.is_finished() {
        escalate_registered_child(id);
        thread::sleep(Duration::from_millis(10));
    }
    join_forwarder(Some(forwarder), program, stream_name)
}

/// Waits for `id` to exit, terminating it once `bound` elapses.
pub(super) fn wait_for_registered_child_with_bound(
    id: u64,
    bound: Duration,
    program: &str,
) -> Result<ExitStatus, SyncError> {
    let deadline = Instant::now() + bound;
    loop {
        let observed = poll_registered_child(id).map_err(|err| SyncError::Spawn {
            program: program.to_owned(),
            message: err.to_string(),
        })?;
        if let Some(status) = observed {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            terminate_registered_child(id);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Runs `command` with an optional wall-clock bound.
///
/// The child is registered under `scope`, so cancellation can reach it, and a
/// child that outlives the bound is terminated and reaped before the output is
/// returned. Its exit code is then absent.
pub(super) fn run_with_deadline(
    mut command: Command,
    program: &str,
    bound: Duration,
    scope: Option<u64>,
) -> Result<CommandOutput, SyncError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let deadline = Instant::now() + bound;
    let mut child = command.spawn().map_err(|err| SyncError::Spawn {
        program: program.to_owned(),
        message: err.to_string(),
    })?;
    let stdout_handle = child
        .stdout
        .take()
        .map(|stdout| forward_stream(stdout, false, program));
    let stderr_handle = child
        .stderr
        .take()
        .map(|stderr| forward_stream(stderr, true, program));
    let (id, already_cancelled) = register_child(scope, child, false);
    let _registration = Registration { id };
    if already_cancelled {
        terminate_registered_child(id);
    }
    let status = loop {
        if let Some(status) = poll_registered_child(id).map_err(|err| SyncError::Spawn {
            program: program.to_owned(),
            message: err.to_string(),
        })? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_registered_child(id);
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = join_forwarder_escalating(stdout_handle, program, "stdout", id)?;
    let stderr = join_forwarder_escalating(stderr_handle, program, "stderr", id)?;
    Ok(CommandOutput {
        code: status.code(),
        stdout,
        stderr,
    })
}
