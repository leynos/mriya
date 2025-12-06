//! Core sync types and command runner abstraction.

use std::ffi::OsString;
use std::io::{BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::thread;

use crate::sync::SyncError;

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
    pub const fn is_success(&self) -> bool {
        matches!(self.code, Some(0))
    }
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

fn forward_stream(
    reader: impl Read + Send + 'static,
    target: StreamTarget,
) -> thread::JoinHandle<Result<String, SyncError>> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut captured = Vec::new();
        let mut buffered_reader = BufReader::new(reader);
        let program = target.program_name();

        target
            .with_locked_writer(|writer| {
                let convert_error = |err: std::io::Error| SyncError::Spawn {
                    program: program.to_owned(),
                    message: err.to_string(),
                };

                let mut read = buffered_reader.read(&mut buffer).map_err(&convert_error)?;
                while read != 0 {
                    let chunk = buffer.get(..read).unwrap_or(&[]);
                    writer.write_all(chunk).map_err(&convert_error)?;
                    captured.extend_from_slice(chunk);
                    read = buffered_reader.read(&mut buffer).map_err(&convert_error)?;
                }

                writer.flush().map_err(&convert_error)?;
                Ok(())
            })
            .map(|()| String::from_utf8_lossy(&captured).into_owned())
    })
}

enum StreamTarget {
    Stdout,
    Stderr,
}

impl StreamTarget {
    const fn program_name(&self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }

    fn with_locked_writer<F, R>(&self, f: F) -> Result<R, SyncError>
    where
        F: FnOnce(&mut dyn Write) -> Result<R, SyncError>,
    {
        match self {
            Self::Stdout => {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                f(&mut handle)
            }
            Self::Stderr => {
                let stderr = std::io::stderr();
                let mut handle = stderr.lock();
                f(&mut handle)
            }
        }
    }
}

/// Command runner that streams subprocess stdout/stderr while capturing them.
#[derive(Clone, Debug, Default)]
pub struct StreamingCommandRunner;

impl CommandRunner for StreamingCommandRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        let mut child = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| SyncError::Spawn {
                program: program.to_owned(),
                message: err.to_string(),
            })?;

        let stdout_handle = child
            .stdout
            .take()
            .map(|stdout| forward_stream(stdout, StreamTarget::Stdout));
        let stderr_handle = child
            .stderr
            .take()
            .map(|stderr| forward_stream(stderr, StreamTarget::Stderr));

        let status = child.wait().map_err(|err| SyncError::Spawn {
            program: program.to_owned(),
            message: err.to_string(),
        })?;

        let stdout = match stdout_handle {
            Some(handle) => handle.join().map_err(|_| SyncError::Spawn {
                program: program.to_owned(),
                message: String::from("stdout forwarder panicked"),
            })??,
            None => String::new(),
        };

        let stderr = match stderr_handle {
            Some(handle) => handle.join().map_err(|_| SyncError::Spawn {
                program: program.to_owned(),
                message: String::from("stderr forwarder panicked"),
            })??,
            None => String::new(),
        };

        Ok(CommandOutput {
            code: status.code(),
            stdout,
            stderr,
        })
    }
}
