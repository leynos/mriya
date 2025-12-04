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
use shell_escape::unix::escape;
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
        let remote_cmd_wrapped = self.build_remote_command(remote_command);
        let args = self.build_ssh_args(networking, &remote_cmd_wrapped);
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
mod tests {
    use super::*;
    use crate::backend::InstanceNetworking;
    use crate::test_support::ScriptedRunner;
    use std::net::{IpAddr, Ipv4Addr};
    use tempfile::TempDir;

    /// Helper to assert validation rejects empty or whitespace values for a
    /// given field.
    fn assert_validation_rejects_field<F>(field_name: &str, set_field: F)
    where
        F: Fn(&mut SyncConfig, String),
    {
        for invalid in ["", "  "] {
            let mut cfg = base_config();
            set_field(&mut cfg, invalid.to_owned());
            let Err(err) = cfg.validate() else {
                panic!("{field_name} '{invalid}' should fail");
            };
            let SyncError::InvalidConfig { ref field } = err else {
                panic!("expected InvalidConfig for {field_name}, got {err:?}");
            };
            assert_eq!(field, field_name, "expected invalid field {field_name}");
        }
    }

    fn base_config() -> SyncConfig {
        SyncConfig {
            rsync_bin: String::from("rsync"),
            ssh_bin: String::from("ssh"),
            ssh_user: String::from("ubuntu"),
            remote_path: String::from("/remote/path"),
            ssh_batch_mode: true,
            ssh_strict_host_key_checking: false,
            ssh_known_hosts_file: String::from("/dev/null"),
        }
    }

    fn networking() -> InstanceNetworking {
        InstanceNetworking {
            public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            ssh_port: 2222,
        }
    }

    #[test]
    fn sync_config_validate_accepts_defaults() {
        let cfg = base_config();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn sync_config_validation_rejects_rsync_bin() {
        assert_validation_rejects_field("rsync_bin", |cfg, val| cfg.rsync_bin = val);
    }

    #[test]
    fn sync_config_validation_rejects_ssh_bin() {
        assert_validation_rejects_field("ssh_bin", |cfg, val| cfg.ssh_bin = val);
    }

    #[test]
    fn sync_config_validation_rejects_ssh_user() {
        assert_validation_rejects_field("ssh_user", |cfg, val| cfg.ssh_user = val);
    }

    #[test]
    fn sync_config_validation_rejects_remote_path() {
        assert_validation_rejects_field("remote_path", |cfg, val| cfg.remote_path = val);
    }

    #[test]
    fn remote_destination_builds_expected_values() {
        let cfg = SyncConfig {
            ssh_user: String::from("alice"),
            remote_path: String::from("/dst"),
            ..base_config()
        };
        let dest = cfg.remote_destination(&networking());
        let SyncDestination::Remote {
            user,
            host,
            port,
            path,
        } = dest
        else {
            panic!("expected remote destination");
        };
        assert_eq!(user, "alice");
        assert_eq!(host, Ipv4Addr::LOCALHOST.to_string());
        assert_eq!(port, 2222);
        assert_eq!(path, Utf8PathBuf::from("/dst"));
    }

    #[test]
    fn build_rsync_args_remote_includes_gitignore_filter() {
        let cfg = base_config();
        let runner = ScriptedRunner::new();
        let syncer = Syncer::new(cfg, runner).expect("config should validate");
        let destination = SyncDestination::Remote {
            user: String::from("ubuntu"),
            host: String::from("1.2.3.4"),
            port: 2222,
            path: Utf8PathBuf::from("/remote"),
        };
        let source_dir = TempDir::new().expect("temp dir");
        let source =
            Utf8PathBuf::from_path_buf(source_dir.path().to_path_buf()).expect("utf8 path");
        let args = syncer
            .build_rsync_args(&source, &destination)
            .expect("args should build");

        let args_strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args_strs.contains(&String::from("--filter=:- .gitignore")));
        assert!(args_strs.contains(&String::from("--exclude")));
        assert!(args_strs.contains(&String::from(".git/")));
        assert!(
            args_strs.iter().any(|arg| arg.starts_with("--rsh")),
            "expected --rsh wrapper"
        );
        assert!(
            args_strs.iter().any(|arg| arg.contains("ssh -p 2222")),
            "expected ssh port in remote shell: {args_strs:?}"
        );
    }

    #[test]
    fn build_rsync_args_local_omits_remote_shell() {
        let cfg = base_config();
        let runner = ScriptedRunner::new();
        let syncer = Syncer::new(cfg, runner).expect("config should validate");
        let destination = SyncDestination::Local {
            path: Utf8PathBuf::from("/tmp/dst"),
        };
        let source_dir = TempDir::new().expect("temp dir");
        let source =
            Utf8PathBuf::from_path_buf(source_dir.path().to_path_buf()).expect("utf8 path");
        let args = syncer
            .build_rsync_args(&source, &destination)
            .expect("args should build");
        let args_strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args_strs.iter().any(|arg| arg == "--rsh"),
            "local sync should not set --rsh"
        );
        assert_eq!(args_strs.last().map(String::as_str), Some("/tmp/dst"));
    }

    #[test]
    fn sync_returns_error_on_non_zero_rsync_status() {
        let cfg = base_config();
        let runner = ScriptedRunner::new();
        runner.push_failure(12);
        let syncer = Syncer::new(cfg, runner).expect("config should validate");
        let destination = SyncDestination::Local {
            path: Utf8PathBuf::from("/tmp/dst"),
        };
        let err = syncer
            .sync(Utf8Path::new("/"), &destination)
            .expect_err("non-zero rsync should error");
        let SyncError::CommandFailure {
            status,
            status_text,
            ..
        } = err
        else {
            panic!("expected CommandFailure");
        };
        assert_eq!(status, Some(12));
        assert_eq!(status_text, "12");
    }

    #[test]
    fn sync_succeeds_on_zero_status() {
        let cfg = base_config();
        let runner = ScriptedRunner::new();
        runner.push_success();
        let syncer = Syncer::new(cfg, runner).expect("config should validate");
        let destination = SyncDestination::Local {
            path: Utf8PathBuf::from("/tmp/dst"),
        };
        assert!(syncer.sync(Utf8Path::new("/"), &destination).is_ok());
    }

    #[test]
    fn run_remote_returns_missing_exit_code() {
        let cfg = base_config();
        let runner = ScriptedRunner::new();
        runner.push_missing_exit_code();
        let syncer = Syncer::new(cfg, runner).expect("config should validate");
        let err = syncer
            .run_remote(&networking(), "echo ok")
            .expect_err("missing exit code should error");
        assert!(matches!(err, SyncError::MissingExitCode { program } if program == "ssh"));
    }

    #[test]
    fn run_remote_propagates_exit_code() {
        let cfg = base_config();
        let runner = ScriptedRunner::new();
        runner.push_exit_code(7);
        let syncer = Syncer::new(cfg, runner).expect("config should validate");
        let output = syncer
            .run_remote(&networking(), "echo ok")
            .unwrap_or_else(|err| panic!("run_remote should succeed: {err}"));
        assert_eq!(output.exit_code, 7);
        assert_eq!(output.stdout, "");
    }

    #[test]
    fn run_remote_cd_prefixes_remote_path() {
        let cfg = base_config();
        let runner = ScriptedRunner::new();
        runner.push_success();
        let syncer = Syncer::new(cfg, runner).expect("config should validate");
        let _ = syncer
            .run_remote(&networking(), "cargo test")
            .expect("run_remote should succeed");

        let args = syncer.build_remote_command("cargo test");
        assert!(
            args.starts_with("cd /remote/path && cargo test"),
            "remote command should change directory, got: {args}"
        );
    }

    #[test]
    fn build_ssh_args_uses_wrapped_command_verbatim() {
        let cfg = base_config();
        let runner = ScriptedRunner::new();
        runner.push_success();
        let syncer = Syncer::new(cfg, runner).expect("config should validate");
        let wrapped = syncer.build_remote_command("echo ok");
        let args = syncer.build_ssh_args(&networking(), &wrapped);

        assert_eq!(
            args.last(),
            Some(&OsString::from(wrapped.clone())),
            "ssh args should forward the already wrapped remote command"
        );
    }
}
