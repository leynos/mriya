//! Core sync types and command runner abstraction.

use std::{
    ffi::OsString,
    io::{self, BufReader, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Mutex,
        MutexGuard,
        PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
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
/// Returns an empty string when no forwarder was spawned for the stream.
fn join_forwarder(
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

/// How often a waiting runner checks whether its child has exited.
///
/// The wait polls rather than blocking so the same [`Child`] can be terminated
/// by the signal listener, which needs the registry lock the waiter would
/// otherwise hold for the whole command.
const CHILD_WAIT_INTERVAL: Duration = Duration::from_millis(10);

/// A streaming child that cancellation may terminate.
#[derive(Debug)]
struct RegisteredChild {
    /// Identifier of this registration.
    id: u64,
    /// Operation scope this child belongs to.
    scope: Option<u64>,
    /// The running child process.
    child: Child,
}

/// Identifier source for registrations, unique within the process.
static NEXT_CHILD_ID: AtomicU64 = AtomicU64::new(1);

/// Streaming children that are currently running.
static ACTIVE_CHILDREN: Mutex<Vec<RegisteredChild>> = Mutex::new(Vec::new());

/// Operation scopes for which cancellation has already been requested.
///
/// A child spawned just before the sweep, but registered just after it, checks
/// this set at registration time and is terminated immediately. Without it,
/// that window would let a command outlive the cancellation that was meant to
/// stop it.
static CANCELLED_SCOPES: Mutex<Vec<Option<u64>>> = Mutex::new(Vec::new());

/// Terminates the streaming children belonging to `scope`.
///
/// A child that is already gone is ignored: the registry keeps the entry until
/// its own waiter reaps it, so a signal that races a natural exit is harmless.
/// Children registered for another operation are left alone.
pub fn kill_active_children(scope: Option<u64>) {
    {
        let mut cancelled = CANCELLED_SCOPES
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !cancelled.contains(&scope) {
            cancelled.push(scope);
        }
    }

    let mut children = lock_children();
    for registered in children.iter_mut().filter(|entry| entry.scope == scope) {
        // A child may have exited between the signal and this call; its own
        // waiter still observes and reports the exit status.
        kill_process_group(&registered.child);
    }
}

/// Requests termination of `child` and every process in its group.
///
/// A launcher can start descendants that inherit stdout or stderr; killing only
/// the launcher would leave those descendants holding the pipes open, and the
/// output forwarders would wait for EOF forever.
fn kill_process_group(child: &Child) {
    #[cfg(unix)]
    {
        // SAFETY: `child` is a running child this process spawned into its own
        // process group, so its pid is a valid process-group id, and `killpg`
        // only sends a signal. `child.id()` cannot exceed a `pid_t` because it
        // came from `fork`.
        let Ok(pid) = libc::pid_t::try_from(child.id()) else {
            return;
        };
        // SAFETY: the pid is a positive process-group id owned by this
        // process, and `killpg` only sends a signal.
        unsafe {
            libc::killpg(pid, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        drop(child.kill());
    }
}

/// Terminates the registered child `id`, if it is still registered.
fn kill_registered_child(id: u64) {
    let mut children = lock_children();
    if let Some(entry) = children.iter_mut().find(|entry| entry.id == id) {
        kill_process_group(&entry.child);
    }
}

/// Locks the streaming-child registry, recovering a poisoned lock.
///
/// The registry holds no invariants that a panic can corrupt beyond the list
/// itself, so losing a registration because another thread panicked would
/// leave an unkillable child; recovery keeps it usable.
fn lock_children() -> MutexGuard<'static, Vec<RegisteredChild>> {
    ACTIVE_CHILDREN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Registers `child` so a signal can terminate it.
///
/// Returns the registration id and whether cancellation had already been
/// requested for `scope`. The caller kills and reaps the child in that case
/// instead of entering the normal wait.
fn register_child(scope: Option<u64>, child: Child) -> (u64, bool) {
    let id = NEXT_CHILD_ID.fetch_add(1, Ordering::Relaxed);
    let cancelled = CANCELLED_SCOPES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&scope);
    lock_children().push(RegisteredChild { id, scope, child });
    (id, cancelled)
}

/// Waits until the registered child exits, allowing cancellation to kill it
/// meanwhile.
fn wait_for_registered_child(id: u64) -> io::Result<ExitStatus> {
    loop {
        if let Some(status) = try_wait_registered_child(id)? {
            return Ok(status);
        }
        thread::sleep(CHILD_WAIT_INTERVAL);
    }
}

/// Polls the registered child once, dropping its registration when it is gone.
///
/// A failed wait leaves the child's state unknown, so the registration is
/// removed rather than left for a later signal to act on.
fn try_wait_registered_child(id: u64) -> io::Result<Option<ExitStatus>> {
    let observed = {
        let mut children = lock_children();
        let Some(entry) = children.iter_mut().find(|entry| entry.id == id) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "streaming child disappeared from the registry",
            ));
        };
        entry.child.try_wait()
    };
    let finished = match observed {
        Ok(None) => return Ok(None),
        Ok(Some(status)) => Ok(status),
        Err(error) => Err(error),
    };
    lock_children().retain(|entry| entry.id != id);
    finished.map(Some)
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
    #[must_use]
    pub const fn new(scope: Option<u64>) -> Self { Self { scope } }
}

impl CommandRunner for StreamingCommandRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Put the child in its own process group, so cancellation can stop the
        // launcher and any descendants it started.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
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

        let (id, already_cancelled) = register_child(self.scope, child);
        if already_cancelled {
            // Cancellation won the race between spawn and registration.
            kill_registered_child(id);
        }
        let status = wait_for_registered_child(id).map_err(|err| SyncError::Spawn {
            program: program.to_owned(),
            message: err.to_string(),
        })?;

        let stdout = join_forwarder(stdout_handle, program, "stdout")?;
        let stderr = join_forwarder(stderr_handle, program, "stderr")?;

        Ok(CommandOutput {
            code: status.code(),
            stdout,
            stderr,
        })
    }
}
