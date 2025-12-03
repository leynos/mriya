//! Git-aware rsync synchronisation and remote command execution.
//!
//! The sync module shells out to the system `rsync` binary, applying
//! `.gitignore` rules so that ignored paths are neither uploaded nor deleted on
//! the remote side. It also provides a thin wrapper around the system `ssh`
//! client to execute a command after synchronisation and return its exit code.

use std::ffi::OsString;
use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use ortho_config::OrthoConfig;
use serde::Deserialize;
use thiserror::Error;

use crate::backend::InstanceNetworking;

/// Default remote working directory used for rsync.
pub const DEFAULT_REMOTE_PATH: &str = "/home/ubuntu/project";

/// Synchronisation and SSH settings loaded via `ortho-config`.
#[derive(Clone, Debug, Deserialize, OrthoConfig, PartialEq, Eq)]
#[ortho_config(prefix = "MRIYA_SYNC")]
pub struct SyncConfig {
    /// Path to the `rsync` executable.
    #[ortho_config(default = "rsync".to_owned())]
    pub rsync_bin: String,
    /// Path to the `ssh` executable.
    #[ortho_config(default = "ssh".to_owned())]
    pub ssh_bin: String,
    /// Remote user to connect as.
    #[ortho_config(default = "root".to_owned())]
    pub ssh_user: String,
    /// Remote path to receive the repository contents.
    #[ortho_config(default = DEFAULT_REMOTE_PATH.to_owned())]
    pub remote_path: String,
}

impl SyncConfig {
    /// Ensures configuration values are present after trimming whitespace.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::InvalidConfig`] when any required field is empty.
    pub fn validate(&self) -> Result<(), SyncError> {
        Self::require_value(&self.rsync_bin, "rsync_bin")?;
        Self::require_value(&self.ssh_bin, "ssh_bin")?;
        Self::require_value(&self.ssh_user, "ssh_user")?;
        Self::require_value(&self.remote_path, "remote_path")?;
        Ok(())
    }

    /// Builds a remote destination using the supplied networking details.
    #[must_use]
    pub fn remote_destination(&self, networking: &InstanceNetworking) -> SyncDestination {
        SyncDestination::Remote {
            user: self.ssh_user.clone(),
            host: networking.public_ip.to_string(),
            port: networking.ssh_port,
            path: Utf8PathBuf::from(&self.remote_path),
        }
    }

    fn require_value(value: &str, field: &str) -> Result<(), SyncError> {
        if value.trim().is_empty() {
            return Err(SyncError::InvalidConfig {
                field: field.to_owned(),
            });
        }
        Ok(())
    }
}

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
        /// Destination path for the synchronised content.
        path: Utf8PathBuf,
    },
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

/// Output captured from a remote command executed over SSH.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteCommandOutput {
    /// Exit code reported by the remote command.
    pub exit_code: i32,
    /// Captured standard output stream.
    pub stdout: String,
    /// Captured standard error stream.
    pub stderr: String,
}

/// Errors surfaced while performing synchronisation or remote execution.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SyncError {
    /// Raised when configuration is missing required values.
    #[error("invalid sync configuration: missing {field}")]
    InvalidConfig {
        /// Configuration field that failed validation.
        field: String,
    },
    /// Raised when the source directory does not exist.
    #[error("sync source directory missing: {path}")]
    MissingSource {
        /// Path that was expected to be synchronised.
        path: Utf8PathBuf,
    },
    /// Raised when a command cannot be spawned.
    #[error("failed to spawn {program}: {message}")]
    Spawn {
        /// Command that failed to start.
        program: String,
        /// Operating system error string.
        message: String,
    },
    /// Raised when `rsync` completes with a non-zero exit code.
    #[error("{program} exited with status {status_text}: {stderr}")]
    CommandFailure {
        /// Command name used for the attempted operation.
        program: String,
        /// Exit status as reported by the OS.
        status: Option<i32>,
        /// Human readable representation of the exit status.
        status_text: String,
        /// Stderr captured from the process.
        stderr: String,
    },
    /// Raised when the SSH command finishes without yielding an exit status.
    #[error("{program} did not return an exit code")]
    MissingExitCode {
        /// Command that completed without a status.
        program: String,
    },
}

/// Orchestrates rsync plus remote execution.
#[derive(Debug)]
pub struct Syncer<R: CommandRunner> {
    config: SyncConfig,
    runner: R,
}

impl Syncer<ProcessCommandRunner> {
    /// Convenience constructor that wires the real process runner.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::InvalidConfig`] when validation fails.
    pub fn with_process_runner(config: SyncConfig) -> Result<Self, SyncError> {
        Self::new(config, ProcessCommandRunner)
    }
}

impl<R: CommandRunner> Syncer<R> {
    /// Creates a new syncer using the provided runner and configuration.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::InvalidConfig`] when configuration validation
    /// fails.
    pub fn new(config: SyncConfig, runner: R) -> Result<Self, SyncError> {
        config.validate()?;
        Ok(Self { config, runner })
    }

    /// Runs git-aware rsync from `source` to the chosen destination.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::MissingSource`] when the source directory is
    /// absent, or [`SyncError::CommandFailure`] if `rsync` returns a non-zero
    /// exit code.
    pub fn sync(&self, source: &Utf8Path, destination: &SyncDestination) -> Result<(), SyncError> {
        let args = self.build_rsync_args(source, destination)?;
        let output = self.runner.run(&self.config.rsync_bin, &args)?;
        if output.is_success() {
            return Ok(());
        }

        let status_text = output
            .code
            .map_or_else(|| String::from("unknown"), |code| code.to_string());
        Err(SyncError::CommandFailure {
            program: self.config.rsync_bin.clone(),
            status: output.code,
            status_text,
            stderr: output.stderr,
        })
    }

    /// Performs a sync followed by execution of `remote_command` via SSH.
    ///
    /// # Errors
    ///
    /// Returns any error from [`Syncer::sync`] or [`Syncer::run_remote`].
    pub fn sync_and_run(
        &self,
        source: &Utf8Path,
        networking: &InstanceNetworking,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, SyncError> {
        let destination = self.config.remote_destination(networking);
        self.sync(source, &destination)?;
        self.run_remote(networking, remote_command)
    }

    /// Executes `remote_command` over SSH and returns the remote exit code.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::MissingExitCode`] when the process exits without a
    /// code (for example, when terminated by a signal).
    pub fn run_remote(
        &self,
        networking: &InstanceNetworking,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, SyncError> {
        let args = self.build_ssh_args(networking, remote_command);
        let output = self.runner.run(&self.config.ssh_bin, &args)?;
        let Some(exit_code) = output.code else {
            return Err(SyncError::MissingExitCode {
                program: self.config.ssh_bin.clone(),
            });
        };

        Ok(RemoteCommandOutput {
            exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn build_rsync_args(
        &self,
        source: &Utf8Path,
        destination: &SyncDestination,
    ) -> Result<Vec<OsString>, SyncError> {
        if !source.is_dir() {
            return Err(SyncError::MissingSource {
                path: source.to_path_buf(),
            });
        }

        let mut args = vec![
            OsString::from("-az"),
            OsString::from("--delete"),
            OsString::from("--filter=:- .gitignore"),
            OsString::from("--exclude"),
            OsString::from(".git/"),
        ];

        match destination {
            SyncDestination::Remote {
                user,
                host,
                port,
                path,
            } => {
                let remote_shell = format!(
                    "{} -p {} -o BatchMode=yes -o StrictHostKeyChecking=no -o \
                     UserKnownHostsFile=/dev/null",
                    self.config.ssh_bin, port
                );
                args.push(OsString::from("--rsh"));
                args.push(OsString::from(remote_shell));
                args.push(OsString::from(format!("{source}/")));
                args.push(OsString::from(format!("{user}@{host}:{path}")));
            }
            SyncDestination::Local { path } => {
                args.push(OsString::from(format!("{source}/")));
                args.push(OsString::from(path));
            }
        }

        Ok(args)
    }

    fn build_ssh_args(
        &self,
        networking: &InstanceNetworking,
        remote_command: &str,
    ) -> Vec<OsString> {
        vec![
            OsString::from("-p"),
            OsString::from(networking.ssh_port.to_string()),
            OsString::from("-o"),
            OsString::from("BatchMode=yes"),
            OsString::from("-o"),
            OsString::from("StrictHostKeyChecking=no"),
            OsString::from("-o"),
            OsString::from("UserKnownHostsFile=/dev/null"),
            OsString::from(format!("{}@{}", self.config.ssh_user, networking.public_ip)),
            OsString::from(remote_command),
        ]
    }
}
