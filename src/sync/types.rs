//! Core sync types and command runner abstraction.

use std::{
    ffi::OsString,
    io::{BufReader, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use camino::Utf8PathBuf;

use crate::sync::SyncError;

/// Target for rsync either on a remote host or locally (used for tests).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncDestination {
    /// Remote sync target.
    Remote {
        /// User used to authenticate via SSH.
        user: String,
        /// Hostname or IPv4 address.
        host: String,
        /// SSH port exposed by the instance.
        port: u16,
        /// Path on the remote machine that receives files.
        path: Utf8PathBuf,
    },
    /// Local path used for behavioural tests and dry-runs.
    Local {
        /// Destination path for the synchronized content.
        path: Utf8PathBuf,
    },
}

/// Output captured from a remote command executed over SSH.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteCommandOutput {
    /// Exit code reported by the remote command (`None` when the process exits
    /// without an exit status, for example after being killed by a signal).
    pub exit_code: Option<i32>,
    /// Captured standard output stream.
    pub stdout: String,
    /// Captured standard error stream.
    pub stderr: String,
}

/// Result of running an external command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    /// Exit code reported by the process, if available.
    pub code: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

impl CommandOutput {
    /// Returns `true` when the exit code equals zero.
    #[must_use]
    pub const fn is_success(&self) -> bool { matches!(self.code, Some(0)) }
}

/// Abstraction over command execution to support fakes in tests.
pub trait CommandRunner {
    /// Runs `program` with the given arguments, capturing stdout and stderr.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::Spawn`] if the command cannot be started.
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError>;
}

/// Real command runner that shells out to the host operating system.
#[derive(Clone, Debug, Default)]
pub struct ProcessCommandRunner;

impl CommandRunner for ProcessCommandRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|err| SyncError::Spawn {
                program: program.to_owned(),
                message: err.to_string(),
            })?;

        Ok(CommandOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Spawns a thread that copies `reader` to this process's stderr (or stdout)
/// as it arrives while also capturing it.
///
/// The thread returns the captured text, decoded lossily as UTF-8, or a
/// `Spawn` error naming `program` when reading or writing fails.
fn forward_stream(
    reader: impl Read + Send + 'static,
    is_stderr: bool,
    program: &str,
) -> thread::JoinHandle<Result<String, SyncError>> {
    let program_name = program.to_owned();
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut captured = Vec::new();
        let mut buffered_reader = BufReader::new(reader);
        let mut writer: Box<dyn Write> = if is_stderr {
            Box::new(std::io::stderr().lock())
        } else {
            Box::new(std::io::stdout().lock())
        };

        loop {
            let read = buffered_reader
                .read(&mut buffer)
                .map_err(|err| SyncError::Spawn {
                    program: program_name.clone(),
                    message: err.to_string(),
                })?;

            if read == 0 {
                break;
            }

            let (chunk, _) = buffer.split_at(read);
            writer.write_all(chunk).map_err(|err| SyncError::Spawn {
                program: program_name.clone(),
                message: err.to_string(),
            })?;
            captured.extend_from_slice(chunk);
        }

        writer.flush().map_err(|err| SyncError::Spawn {
            program: program_name.clone(),
            message: err.to_string(),
        })?;

        Ok(String::from_utf8_lossy(&captured).into_owned())
    })
}

/// Join a forwarder thread, mapping panics and stream errors to
/// [`SyncError::Spawn`].
///
/// Returns an empty string when no forwarder was spawned for the stream. With
/// a deadline, the wait is bounded: a descendant that inherited the output
/// pipe can keep it open after the direct child has exited, and joining
/// without a bound would let that descendant hold the attempt past its
/// lifetime.
fn join_forwarder(
    handle: Option<thread::JoinHandle<Result<String, SyncError>>>,
    wait: &ForwarderWait<'_>,
) -> Result<String, SyncError> {
    let Some(join_handle) = handle else {
        return Ok(String::new());
    };
    let Some(limit) = wait.deadline else {
        return join_handle
            .join()
            .map_err(|_| forwarder_panicked(wait.program, wait.stream_name))?;
    };
    loop {
        if join_handle.is_finished() {
            return join_handle
                .join()
                .map_err(|_| forwarder_panicked(wait.program, wait.stream_name))?;
        }
        let now = Instant::now();
        if now >= limit {
            // The direct child may have exited while a descendant held the
            // pipe. Stop the whole group, then give the forwarder a short,
            // bounded chance to observe the closed pipes.
            kill_group(wait.group);
            let _forwarder = wait_for_forwarder(join_handle, wait);
            return Err(SyncError::DeadlineExceeded {
                program: wait.program.to_owned(),
            });
        }
        thread::sleep((limit - now).min(DEADLINE_POLL_INTERVAL));
    }
}

/// The context a bounded forwarder join needs.
struct ForwarderWait<'a> {
    /// Command the forwarder belongs to.
    program: &'a str,
    /// Which stream the forwarder reads, for the panic message.
    stream_name: &'a str,
    /// Absolute deadline the join must not exceed.
    deadline: Option<Instant>,
    /// Process group to stop when the deadline passes.
    group: Option<u32>,
}

/// Waits a short, bounded time for a forwarder after its pipes were closed.
///
/// A forwarder whose pipe is still held open is detached rather than joined:
/// the call has already reached its deadline and must return.
fn wait_for_forwarder(
    handle: thread::JoinHandle<Result<String, SyncError>>,
    wait: &ForwarderWait<'_>,
) -> Result<String, SyncError> {
    let deadline = Instant::now() + FORWARDER_CLEANUP_GRACE;
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(DEADLINE_POLL_INTERVAL);
    }
    if handle.is_finished() {
        return handle
            .join()
            .map_err(|_| forwarder_panicked(wait.program, wait.stream_name))?;
    }
    Ok(String::new())
}

/// Builds the error a panicked forwarder reports.
fn forwarder_panicked(program: &str, stream_name: &str) -> SyncError {
    SyncError::Spawn {
        program: program.to_owned(),
        message: format!("{stream_name} forwarder panicked"),
    }
}

/// How often a deadline-bounded wait checks whether its child has exited.
const DEADLINE_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long a forwarder is given to finish after its process group is killed.
const FORWARDER_CLEANUP_GRACE: Duration = Duration::from_secs(1);

/// Command runner that streams subprocess stdout/stderr while capturing them.
#[derive(Clone, Debug, Default)]
pub struct StreamingCommandRunner;

impl CommandRunner for StreamingCommandRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        run_streaming(program, args, None)
    }
}

/// Command runner that kills any child which outlives an absolute deadline.
///
/// The benchmark campaign uses this so a stalled `ssh` or `rsync` cannot hold
/// an attempt open past the lifetime its operator declared. The bound is
/// absolute rather than per call: every command the attempt runs shares the
/// deadline, so preparation, probes, synchronization and the workload together
/// cannot exceed it.
#[derive(Clone, Debug)]
pub struct DeadlineCommandRunner {
    deadline: Instant,
}

impl DeadlineCommandRunner {
    /// Creates a runner whose children must finish before `deadline`.
    #[must_use]
    pub const fn new(deadline: Instant) -> Self { Self { deadline } }
}

impl CommandRunner for DeadlineCommandRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        run_streaming(program, args, Some(self.deadline))
    }
}

/// Runs a child, forwarding its streams, and waits for it.
///
/// With a deadline, the child is killed once the deadline passes, so the wait
/// cannot outlive the caller's budget even when the child never exits. The
/// bound is checked before spawning as well, so a phase that has already spent
/// its budget starts no process at all.
fn run_streaming(
    program: &str,
    args: &[OsString],
    deadline: Option<Instant>,
) -> Result<CommandOutput, SyncError> {
    if deadline.is_some_and(|limit| Instant::now() >= limit) {
        return Err(SyncError::DeadlineExceeded {
            program: program.to_owned(),
        });
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A bounded command gets its own process group, so a timeout can stop the
    // descendants it started rather than only the direct child.
    #[cfg(unix)]
    if deadline.is_some() {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|err| spawn_error(program, &err))?;
    let group = deadline.map(|_| child.id());

    let stdout_handle = child
        .stdout
        .take()
        .map(|stdout| forward_stream(stdout, false, program));
    let stderr_handle = child
        .stderr
        .take()
        .map(|stderr| forward_stream(stderr, true, program));

    let status = match deadline {
        Some(limit) => wait_bounded(&mut child, limit, program, group)?,
        None => child.wait().map_err(|err| spawn_error(program, &err))?,
    };

    let stdout = join_forwarder(
        stdout_handle,
        &ForwarderWait {
            program,
            stream_name: "stdout",
            deadline,
            group,
        },
    )?;
    let stderr = join_forwarder(
        stderr_handle,
        &ForwarderWait {
            program,
            stream_name: "stderr",
            deadline,
            group,
        },
    )?;

    Ok(CommandOutput {
        code: status.code(),
        stdout,
        stderr,
    })
}

/// Waits for `child`, killing it once `deadline` passes.
///
/// A child that exits between the status check and the kill is harmless: the
/// deadline has already passed, so the caller would discard the result anyway.
fn wait_bounded(
    child: &mut Child,
    deadline: Instant,
    program: &str,
    group: Option<u32>,
) -> Result<ExitStatus, SyncError> {
    loop {
        if let Some(status) = child.try_wait().map_err(|err| spawn_error(program, &err))? {
            return Ok(status);
        }
        let now = Instant::now();
        if now >= deadline {
            kill_tree(child, group);
            return Err(SyncError::DeadlineExceeded {
                program: program.to_owned(),
            });
        }
        thread::sleep((deadline - now).min(DEADLINE_POLL_INTERVAL));
    }
}

/// Stops a bounded child and every descendant that stayed in its group.
fn kill_tree(child: &mut Child, group: Option<u32>) {
    kill_group(group);
    child.kill().ok();
    child.wait().ok();
}

/// Sends `SIGKILL` to the process group `group`, when one was created.
#[cfg(unix)]
fn kill_group(group: Option<u32>) {
    let Some(pgid) = group else {
        return;
    };
    // `kill -KILL -<pgid>` targets the whole group, which is how descendants
    // that inherited the output pipes are stopped too.
    Command::new("kill")
        .arg("-KILL")
        .arg(format!("-{pgid}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok();
}

/// Non-Unix builds cannot create a process group here, so no group is killed.
#[cfg(not(unix))]
fn kill_group(_group: Option<u32>) {}

/// Builds the error a failed spawn or wait reports.
fn spawn_error(program: &str, err: &std::io::Error) -> SyncError {
    SyncError::Spawn {
        program: program.to_owned(),
        message: err.to_string(),
    }
}
