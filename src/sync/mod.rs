//! Git-aware rsync synchronization and remote command execution applying
//! `.gitignore` filters and wrapping SSH commands while preserving remote
//! exit codes.

use std::ffi::OsString;

use camino::Utf8Path;
use mockable::{DefaultEnv, Env};

use crate::backend::InstanceNetworking;

mod config;
mod remote_command;
mod types;
mod util;

pub use camino::Utf8PathBuf;
pub use config::{
    DEFAULT_REMOTE_PATH, DEFAULT_VOLUME_MOUNT_PATH, SyncConfig, SyncConfigLoadError, SyncError,
};
pub use remote_command::{CACHE_SUBDIRECTORIES, create_cache_directories_command};
pub use types::{
    CommandOutput, CommandRunner, ProcessCommandRunner, RemoteCommandOutput,
    StreamingCommandRunner, SyncDestination,
};
pub use util::{HomeUnavailable, expand_tilde, expand_tilde_with_home, home_from_env};

/// Orchestrates rsync plus remote execution.
#[derive(Clone, Debug)]
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

    /// Returns a reference to the underlying configuration.
    #[must_use]
    pub const fn config(&self) -> &SyncConfig {
        &self.config
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
    /// directory prefix; callers must ensure any untrusted input is sanitized
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
        self.execute_ssh(networking, &remote_cmd_wrapped)
    }

    /// Executes `remote_command` over SSH without applying the working
    /// directory prefix or cache routing preamble.
    ///
    /// # Errors
    ///
    /// Propagates any failure to spawn or execute the SSH command from the
    /// configured [`CommandRunner`].
    ///
    /// # Security
    ///
    /// `remote_command` is passed verbatim to the SSH client. Ensure any
    /// caller-provided arguments are validated or quoted upstream.
    pub fn run_remote_raw(
        &self,
        networking: &InstanceNetworking,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, SyncError> {
        self.execute_ssh(networking, remote_command)
    }

    fn execute_ssh(
        &self,
        networking: &InstanceNetworking,
        command: &str,
    ) -> Result<RemoteCommandOutput, SyncError> {
        let args = self.build_ssh_args(networking, command, &DefaultEnv)?;
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
                let remote_shell = self.build_remote_shell(*port, &DefaultEnv)?;
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

    /// Builds the `ssh` argument list for one remote command.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::HomeUnavailable`] when the configured identity
    /// file starts with `~/` and `HOME` cannot be decoded, because
    /// `ssh` would otherwise be handed the literal path `~/…` and fail for a
    /// reason the user cannot act on.
    fn build_ssh_args(
        &self,
        networking: &InstanceNetworking,
        remote_command: &str,
        env: &impl Env,
    ) -> Result<Vec<OsString>, SyncError> {
        let mut args = self.common_ssh_options(networking.ssh_port, env)?;
        args.push(OsString::from(format!(
            "{}@{}",
            self.config.ssh_user, networking.public_ip
        )));
        args.push(OsString::from(remote_command));
        Ok(args)
    }

    /// Builds the SSH options shared by `ssh` and the rsync remote shell.
    ///
    /// Reading `HOME` here, rather than inside the expansion, is what keeps
    /// [`expand_tilde_with_home`] free of I/O: this is the boundary that owns
    /// the process-environment lookup, and it reports the failure at the
    /// boundary instead of passing an unexpanded path down to `ssh`.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::HomeUnavailable`] when the identity file needs a
    /// `HOME` that `env` cannot decode; the error names the configured file
    /// so an undecodable environment is not mistaken for a missing key.
    fn common_ssh_options(&self, port: u16, env: &impl Env) -> Result<Vec<OsString>, SyncError> {
        let mut args = vec![OsString::from("-p"), OsString::from(port.to_string())];

        if let Some(ref identity_file) = self.config.ssh_identity_file {
            let home =
                home_from_env(env, identity_file).map_err(|source| SyncError::HomeUnavailable {
                    path: identity_file.clone(),
                    source,
                })?;
            let expanded = expand_tilde_with_home(identity_file, home.as_deref());
            args.push(OsString::from("-i"));
            args.push(OsString::from(expanded));
        }

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

        Ok(args)
    }

    /// Builds the `--rsh` value rsync uses to reach the instance.
    ///
    /// The remote shell is one command string, so it shares the option set
    /// `ssh` itself receives rather than growing a second copy of it; the
    /// identity flag is therefore expanded here exactly as it is for `ssh`.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError::HomeUnavailable`] for the same reason as
    /// [`Syncer::common_ssh_options`], because this delegates to it.
    fn build_remote_shell(&self, port: u16, env: &impl Env) -> Result<String, SyncError> {
        let opts = self
            .common_ssh_options(port, env)?
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(format!("{} {}", self.config.ssh_bin, opts))
    }

    fn build_remote_command(&self, remote_command: &str) -> String {
        remote_command::build_remote_command(&self.config, remote_command)
    }
}

#[cfg(test)]
mod tests;
