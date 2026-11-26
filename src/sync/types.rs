//! Core sync types and command runner abstraction.

use std::{
    ffi::OsString,
    io::{BufReader, Read, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use camino::Utf8PathBuf;

#[cfg(unix)]
use super::registry::should_isolate_group;
use super::{
    registry::{
        Registration,
        register_child,
        terminate_registered_child,
        wait_for_registered_child,
    },
    runner::{join_forwarder_escalating, run_with_deadline, wait_for_registered_child_with_bound},
};
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

    /// Runs `program` with a wall-clock bound.
    ///
    /// # Parameters
    ///
    /// - `program`: executable to start.
    /// - `args`: arguments passed verbatim.
    /// - `bound`: maximum wall-clock time before the child is terminated.
    ///
    /// # Returns
    ///
    /// The captured output. A child terminated at the bound has no exit code.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::Spawn`] if the command cannot be started. The
    /// default implementation cannot enforce the bound and simply runs the
    /// command, which is suitable only for test doubles.
    fn run_bounded(
        &self,
        program: &str,
        args: &[OsString],
        bound: Duration,
    ) -> Result<CommandOutput, SyncError> {
        let _ = bound;
        self.run(program, args)
    }
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

    fn run_bounded(
        &self,
        program: &str,
        args: &[OsString],
        bound: Duration,
    ) -> Result<CommandOutput, SyncError> {
        let mut command = Command::new(program);
        command.args(args);
        run_with_deadline(command, program, bound, None)
    }
}

/// Spawns a thread that copies `reader` to this process's stderr (or stdout)
/// as it arrives while also capturing it.
///
/// The thread returns the captured text, decoded lossily as UTF-8, or a
/// `Spawn` error naming `program` when reading or writing fails.
pub(super) fn forward_stream(
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
/// Returns an empty string when no forwarder was spawned for the stream.
pub(super) fn join_forwarder(
    handle: Option<thread::JoinHandle<Result<String, SyncError>>>,
    program: &str,
    stream_name: &str,
) -> Result<String, SyncError> {
    match handle {
        Some(join_handle) => join_handle.join().map_err(|_| SyncError::Spawn {
            program: program.to_owned(),
            message: format!("{stream_name} forwarder panicked"),
        })?,
        None => Ok(String::new()),
    }
}

/// Command runner that streams subprocess stdout/stderr while capturing them.
///
/// The optional scope ties the runner's children to one cancellation channel,
/// so terminating one operation cannot stop a child another operation started.
#[derive(Clone, Debug, Default)]
pub struct StreamingCommandRunner {
    /// Scope this runner's children register under.
    scope: Option<u64>,
}

impl StreamingCommandRunner {
    /// Creates a runner whose children belong to `scope`.
    ///
    /// # Parameters
    ///
    /// - `scope`: the cancellation scope to register children under. `None` makes this an unscoped
    ///   runner, which `kill_active_children` reaches only when called with `None`.
    ///
    /// # Returns
    ///
    /// A runner that registers every child it spawns, so a signal can
    /// terminate the child and the descendants it started.
    #[must_use]
    pub const fn new(scope: Option<u64>) -> Self { Self { scope } }
}

impl CommandRunner for StreamingCommandRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        self.run_with_bound(program, args, None)
    }

    fn run_bounded(
        &self,
        program: &str,
        args: &[OsString],
        bound: Duration,
    ) -> Result<CommandOutput, SyncError> {
        self.run_with_bound(program, args, Some(bound))
    }
}

impl StreamingCommandRunner {
    /// Runs `program`, optionally terminating it once `bound` elapses.
    fn run_with_bound(
        &self,
        program: &str,
        args: &[OsString],
        bound: Option<Duration>,
    ) -> Result<CommandOutput, SyncError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Put the child in its own process group, so cancellation can stop the
        // launcher and any descendants it started. A command that inherits a
        // terminal keeps the foreground group instead: moving it would stop the
        // command on `SIGTTIN` the moment it reads from standard input.
        #[cfg(unix)]
        let isolated = {
            use std::os::unix::process::CommandExt as _;
            let isolate = should_isolate_group();
            if isolate {
                command.process_group(0);
            }
            isolate
        };
        #[cfg(not(unix))]
        let isolated = false;
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

        let (id, already_cancelled) = register_child(self.scope, child, isolated);
        let _registration = Registration { id };
        if already_cancelled {
            // Cancellation won the race between spawn and registration.
            terminate_registered_child(id);
        }
        let status = match bound {
            Some(limit) => wait_for_registered_child_with_bound(id, limit, program)?,
            None => wait_for_registered_child(id).map_err(|err| SyncError::Spawn {
                program: program.to_owned(),
                message: err.to_string(),
            })?,
        };

        // The forwarders finish only once every process holding the pipes has
        // gone, so the registration stays until here: a cancellation during the
        // drain can still reach a descendant left behind by the launcher.
        let stdout = join_forwarder_escalating(stdout_handle, program, "stdout", id)?;
        let stderr = join_forwarder_escalating(stderr_handle, program, "stderr", id)?;

        Ok(CommandOutput {
            code: status.code(),
            stdout,
            stderr,
        })
    }
}
