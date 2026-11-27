//! SSH command execution for the syncer.
//!
//! The remote-command surface lives here so the synchronizer module stays
//! focused on rsync and destination selection.

use std::{ffi::OsString, time::Duration};

use mockable::DefaultEnv;

use super::{CacheRouting, CommandRunner, RemoteCommandOutput, SyncError, Syncer};
use crate::backend::InstanceNetworking;

impl<R: CommandRunner> Syncer<R> {
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
        self.run_remote_with_cache(networking, remote_command, &CacheRouting::Unconfigured)
    }

    /// Executes `remote_command` over SSH with the requested cache routing.
    ///
    /// A caller that attached a configured cache passes
    /// [`CacheRouting::Required`] with the device it verified. The preamble
    /// then fails closed when the path's source is no longer that device, so
    /// build output cannot be silently redirected onto the root filesystem. A
    /// run without a configured cache keeps the guarded historical preamble.
    ///
    /// # Parameters
    ///
    /// - `networking`: public address and SSH port of the instance.
    /// - `remote_command`: command to execute after the optional cache prefix.
    /// - `routing`: whether the command requires a verified cache mount.
    ///
    /// # Returns
    ///
    /// The remote exit code and captured streams.
    ///
    /// # Errors
    ///
    /// Propagates any failure to spawn or execute the SSH command.
    ///
    /// # Security
    ///
    /// `remote_command` is not escaped; only the working directory component is
    /// shell-escaped. Ensure any caller-provided arguments are validated or
    /// quoted upstream.
    pub fn run_remote_with_cache(
        &self,
        networking: &InstanceNetworking,
        remote_command: &str,
        routing: &CacheRouting,
    ) -> Result<RemoteCommandOutput, SyncError> {
        let remote_cmd_wrapped = self.build_remote_command(remote_command, routing);
        self.execute_ssh(networking, &remote_cmd_wrapped)
    }

    /// Executes `remote_command` over SSH, killing the local client after
    /// `limit`.
    ///
    /// `timeout` prefixes the SSH invocation, so a session that stays
    /// responsive while the remote shell never runs the command cannot block
    /// the caller past the limit. A killed session reports status 124.
    ///
    /// # Errors
    ///
    /// Propagates any failure to spawn or execute the bounded command.
    ///
    /// # Security
    ///
    /// `remote_command` is not escaped; only the working directory component is
    /// shell-escaped.
    pub fn run_remote_with_timeout(
        &self,
        networking: &InstanceNetworking,
        remote_command: &str,
        limit: Duration,
    ) -> Result<RemoteCommandOutput, SyncError> {
        let args = self.build_ssh_args(networking, remote_command, &DefaultEnv)?;
        let seconds = limit.as_secs().max(1).to_string();
        let mut bounded = Vec::with_capacity(args.len() + 2);
        bounded.push(OsString::from(seconds));
        bounded.push(OsString::from(&self.config.ssh_bin));
        bounded.extend(args);
        let output = self.runner.run("timeout", &bounded)?;
        Ok(RemoteCommandOutput {
            exit_code: output.code,
            stdout: output.stdout,
            stderr: output.stderr,
        })
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
}
