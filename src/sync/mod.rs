//! Git-aware rsync synchronisation and remote command execution applying
//! `.gitignore` filters and wrapping SSH commands while preserving remote
//! exit codes.

use std::ffi::OsString;

use camino::{Utf8Path, Utf8PathBuf};
use ortho_config::OrthoConfig;
use serde::Deserialize;
use shell_escape::unix::escape;
use thiserror::Error;

use crate::backend::InstanceNetworking;
mod types;
pub use types::{CommandOutput, CommandRunner, ProcessCommandRunner, StreamingCommandRunner};

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
    /// Whether to force batch mode for SSH to avoid password prompts.
    #[ortho_config(default = true)]
    pub ssh_batch_mode: bool,
    /// Whether to enforce host key checking; defaults to disabling to smooth
    /// ephemeral hosts.
    #[ortho_config(default = false)]
    pub ssh_strict_host_key_checking: bool,
    /// Known hosts file override; defaults to `/dev/null` for ephemeral hosts.
    #[ortho_config(default = "/dev/null".to_owned())]
    pub ssh_known_hosts_file: String,
}

/// Errors raised when loading the sync configuration from layered sources.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum SyncConfigLoadError {
    /// Indicates that parsing or merging configuration layers failed.
    #[error("sync configuration parsing failed: {0}")]
    Parse(String),
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

    /// Loads configuration using defaults, configuration files, and
    /// environment variables. CLI overrides are parsed from the provided
    /// iterator.
    ///
    /// # Errors
    ///
    /// Returns [`SyncConfigLoadError::Parse`] when merging sources fails.
    pub fn load_without_cli_args() -> Result<Self, SyncConfigLoadError> {
        Self::load_from_iter([OsString::from("mriya")])
            .map_err(|err| SyncConfigLoadError::Parse(err.to_string()))
    }

    /// Loads configuration using the default argument iterator.
    ///
    /// # Errors
    ///
    /// Returns [`SyncConfigLoadError::Parse`] when merging sources fails.
    pub fn load_from_sources() -> Result<Self, SyncConfigLoadError> {
        Self::load().map_err(|err| SyncConfigLoadError::Parse(err.to_string()))
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
    ///
    /// # Security
    ///
    /// `remote_command` is passed verbatim to the SSH client after the working
    /// directory prefix; callers must ensure any untrusted input is sanitised
    /// before invoking this method.
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

    /// Builds a sync destination using the configured SSH settings.
    #[must_use]
    pub fn destination_for(&self, networking: &InstanceNetworking) -> SyncDestination {
        self.config.remote_destination(networking)
    }

    /// Executes `remote_command` over SSH and returns the remote exit code.
    ///
    /// # Errors
    ///
    /// Propagates any failure to spawn or execute the SSH command from the
    /// configured [`CommandRunner`].
    ///
    /// # Security
    ///
    /// `remote_command` is not escaped; only the working directory component is
    /// shell-escaped. Ensure any caller-provided arguments are validated or
    /// quoted upstream.
    pub fn run_remote(
        &self,
        networking: &InstanceNetworking,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, SyncError> {
        let remote_cmd_wrapped = self.build_remote_command(remote_command);
        let args = self.build_ssh_args(networking, &remote_cmd_wrapped);
        let output = self.runner.run(&self.config.ssh_bin, &args)?;

        Ok(RemoteCommandOutput {
            exit_code: output.code,
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
                let remote_shell = self.build_remote_shell(*port);
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
        let mut args = self.common_ssh_options(networking.ssh_port);
        args.push(OsString::from(format!(
            "{}@{}",
            self.config.ssh_user, networking.public_ip
        )));
        args.push(OsString::from(remote_command));
        args
    }

    fn common_ssh_options(&self, port: u16) -> Vec<OsString> {
        let mut args = vec![OsString::from("-p"), OsString::from(port.to_string())];

        if self.config.ssh_batch_mode {
            args.push(OsString::from("-o"));
            args.push(OsString::from("BatchMode=yes"));
        }

        if !self.config.ssh_strict_host_key_checking {
            args.push(OsString::from("-o"));
            args.push(OsString::from("StrictHostKeyChecking=no"));
        }

        if !self.config.ssh_known_hosts_file.trim().is_empty() {
            args.push(OsString::from("-o"));
            args.push(OsString::from(format!(
                "UserKnownHostsFile={}",
                self.config.ssh_known_hosts_file
            )));
        }

        args
    }

    fn build_remote_shell(&self, port: u16) -> String {
        let opts = self
            .common_ssh_options(port)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} {}", self.config.ssh_bin, opts)
    }

    fn build_remote_command(&self, remote_command: &str) -> String {
        let escaped_path = escape(self.config.remote_path.clone().into());
        format!("cd {escaped_path} && {remote_command}")
    }
}

#[cfg(test)]
mod tests;
