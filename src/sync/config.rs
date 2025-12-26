//! Synchronisation configuration structures and validation.
//!
//! This module defines [`SyncConfig`] for SSH and rsync settings, along with
//! associated error types. Configuration is loaded via `ortho-config` which
//! merges defaults, configuration files, and environment variables.

use camino::Utf8PathBuf;
use ortho_config::OrthoConfig;
use serde::Deserialize;
use thiserror::Error;

use crate::backend::InstanceNetworking;

use super::types::SyncDestination;

/// Default remote working directory used for rsync.
pub const DEFAULT_REMOTE_PATH: &str = "/home/ubuntu/project";

/// Default mount path for the persistent cache volume.
pub const DEFAULT_VOLUME_MOUNT_PATH: &str = "/mriya";

/// Synchronisation and SSH settings loaded via `ortho-config`.
#[derive(Clone, Debug, Deserialize, OrthoConfig, PartialEq, Eq)]
#[ortho_config(
    prefix = "MRIYA_SYNC",
    discovery(
        app_name = "mriya",
        env_var = "MRIYA_CONFIG_PATH",
        config_file_name = "mriya.toml",
        dotfile_name = ".mriya.toml",
        project_file_name = "mriya.toml"
    )
)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "configuration struct with user-facing toggle settings that are naturally expressed as booleans"
)]
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
    /// Path to the SSH private key file for remote authentication. Supports
    /// tilde expansion (`~/.ssh/id_ed25519`). Optional; when not provided, SSH
    /// falls back to default key locations (`~/.ssh/id_rsa`, `~/.ssh/id_ed25519`,
    /// etc.). Validation rejects empty or whitespace-only values.
    pub ssh_identity_file: Option<String>,
    /// Mount path for the persistent cache volume on the remote instance.
    #[ortho_config(default = DEFAULT_VOLUME_MOUNT_PATH.to_owned())]
    pub volume_mount_path: String,
    /// Whether to route common language build caches to the mounted cache
    /// volume when it is available.
    #[ortho_config(default = true)]
    pub route_build_caches: bool,
    /// Whether to create cache subdirectories on the mounted volume after
    /// mounting. Defaults to true so toolchains can write immediately.
    #[ortho_config(default = true)]
    pub create_cache_directories: bool,
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
        Self::require_optional_value(self.ssh_identity_file.as_deref(), "ssh_identity_file")?;
        Self::require_value(&self.volume_mount_path, "volume_mount_path")?;
        Ok(())
    }

    fn require_optional_value(value: Option<&str>, field: &str) -> Result<(), SyncError> {
        match value {
            None => Ok(()), // Not configured; SSH uses defaults
            Some(v) if !v.trim().is_empty() => Ok(()),
            Some(_) => Err(SyncError::InvalidConfig {
                field: field.to_owned(),
            }),
        }
    }

    /// Loads configuration using defaults, configuration files, and
    /// environment variables. CLI overrides are parsed from the provided
    /// iterator.
    ///
    /// # Errors
    ///
    /// Returns [`SyncConfigLoadError::Parse`] when merging sources fails.
    pub fn load_without_cli_args() -> Result<Self, SyncConfigLoadError> {
        Self::load_from_iter([std::ffi::OsString::from("mriya")])
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
        Self::require_optional_value(Some(value), field)
    }
}

/// Errors surfaced while performing synchronisation or remote execution.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SyncError {
    /// Raised when configuration is missing required values. The error message
    /// includes guidance on how to provide the value via environment variable
    /// or configuration file.
    #[error("missing {field}: set MRIYA_SYNC_{env_suffix} or add {field} to [sync] in mriya.toml", env_suffix = field.to_uppercase())]
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
