//! Scaleway test-resource janitor.
//!
//! The janitor is designed for integration tests that provision real cloud
//! resources. It identifies resources belonging to a specific test run via a
//! unique tag (`mriya-test-run-<id>`) and deletes them, failing if anything
//! remains afterwards.

use std::ffi::OsString;

use serde::Deserialize;
use thiserror::Error;

use crate::sync::{CommandOutput, CommandRunner, ProcessCommandRunner, SyncError};

/// Environment variable used by test harnesses to identify a test run.
pub const TEST_RUN_ID_ENV: &str = "MRIYA_TEST_RUN_ID";

/// Prefix used for test run tags applied to Scaleway resources.
pub const TEST_RUN_TAG_PREFIX: &str = "mriya-test-run-";

/// Default Scaleway CLI binary name.
pub const DEFAULT_SCW_BIN: &str = "scw";

/// Configuration for a janitor sweep.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JanitorConfig {
    /// Project id to scope resource discovery.
    pub project_id: String,
    /// Test run identifier used to build the tag.
    pub test_run_id: String,
    /// Path to the `scw` CLI binary.
    pub scw_bin: String,
}

impl JanitorConfig {
    /// Constructs a config, trimming whitespace.
    ///
    /// # Errors
    ///
    /// Returns [`JanitorError::InvalidConfig`] when any required field is blank.
    pub fn new(
        project_id: impl Into<String>,
        test_run_id: impl Into<String>,
        scw_bin: impl Into<String>,
    ) -> Result<Self, JanitorError> {
        let trimmed_project_id = project_id.into().trim().to_owned();
        let trimmed_test_run_id = test_run_id.into().trim().to_owned();
        let trimmed_scw_bin = scw_bin.into().trim().to_owned();
        if trimmed_project_id.is_empty() {
            return Err(JanitorError::InvalidConfig {
                field: String::from("project_id"),
            });
        }
        if trimmed_test_run_id.is_empty() {
            return Err(JanitorError::InvalidConfig {
                field: String::from("test_run_id"),
            });
        }
        if trimmed_scw_bin.is_empty() {
            return Err(JanitorError::InvalidConfig {
                field: String::from("scw_bin"),
            });
        }
        Ok(Self {
            project_id: trimmed_project_id,
            test_run_id: trimmed_test_run_id,
            scw_bin: trimmed_scw_bin,
        })
    }

    /// Returns the full tag used for this test run.
    #[must_use]
    pub fn test_run_tag(&self) -> String {
        format!("{TEST_RUN_TAG_PREFIX}{}", self.test_run_id)
    }
}

/// Summary of janitor work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SweepSummary {
    /// Number of servers deleted during the sweep.
    pub deleted_servers: usize,
    /// Number of Block Storage volumes deleted during the sweep.
    pub deleted_volumes: usize,
}

/// Errors returned by the janitor.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum JanitorError {
    /// Raised when configuration is missing required values.
    #[error("missing {field}")]
    InvalidConfig {
        /// Name of the missing or invalid field.
        field: String,
    },
    /// Raised when `scw` returns a non-zero exit status.
    #[error("{program} exited with status {status_text}: {stderr}")]
    CommandFailure {
        /// Program that failed (typically `scw`).
        program: String,
        /// Exit status reported by the OS.
        status: Option<i32>,
        /// Human readable representation of the exit status.
        status_text: String,
        /// Stderr captured from the command.
        stderr: String,
    },
    /// Raised when JSON output from the CLI cannot be parsed.
    #[error("failed to parse {resource} output: {message}")]
    Parse {
        /// Resource type being parsed (for example `servers`).
        resource: String,
        /// Parser error message.
        message: String,
    },
    /// Raised when resources remain after the sweep.
    #[error("resources remain after janitor sweep: {message}")]
    NotClean {
        /// Human-readable description of what remains.
        message: String,
    },
    /// Raised when command execution fails.
    #[error(transparent)]
    Runner(#[from] SyncError),
}

/// Deletes test-run-tagged Scaleway resources by shelling out to `scw`.
#[derive(Clone, Debug)]
pub struct Janitor<R: CommandRunner> {
    config: JanitorConfig,
    runner: R,
}

impl Janitor<ProcessCommandRunner> {
    /// Creates a janitor wired to the real process runner.
    #[must_use]
    pub const fn with_process_runner(config: JanitorConfig) -> Self {
        Self::new(config, ProcessCommandRunner)
    }
}

impl<R: CommandRunner> Janitor<R> {
    /// Creates a new janitor using the provided configuration and runner.
    #[must_use]
    pub const fn new(config: JanitorConfig, runner: R) -> Self {
        Self { config, runner }
    }

    /// Performs a sweep and returns how many resources were deleted.
    ///
    /// The sweep is ordered: servers are deleted first (waiting for deletion),
    /// then tagged volumes are deleted. The command fails if any tagged
    /// resources remain at the end.
    ///
    /// # Errors
    ///
    /// Returns [`JanitorError`] when `scw` fails, output cannot be parsed, or
    /// resources remain after deletion attempts.
    pub fn sweep(&self) -> Result<SweepSummary, JanitorError> {
        let tag = self.config.test_run_tag();

        let mut deleted_servers = 0;
        let servers = self.list_servers()?;
        for server in servers.iter().filter(|srv| srv.tags.contains(&tag)) {
            self.delete_server(server)?;
            deleted_servers += 1;
        }

        let mut deleted_volumes = 0;
        let volumes = self.list_volumes()?;
        for volume in volumes.iter().filter(|vol| vol.tags.contains(&tag)) {
            self.delete_volume(volume)?;
            deleted_volumes += 1;
        }

        let remaining_servers = self
            .list_servers()?
            .into_iter()
            .filter(|srv| srv.tags.contains(&tag))
            .collect::<Vec<_>>();
        let remaining_volumes = self
            .list_volumes()?
            .into_iter()
            .filter(|vol| vol.tags.contains(&tag))
            .collect::<Vec<_>>();

        if !remaining_servers.is_empty() || !remaining_volumes.is_empty() {
            let message = format!(
                "servers remaining: {}, volumes remaining: {}",
                remaining_servers.len(),
                remaining_volumes.len()
            );
            return Err(JanitorError::NotClean { message });
        }

        Ok(SweepSummary {
            deleted_servers,
            deleted_volumes,
        })
    }

    /// Checks command output and converts failure to `JanitorError`.
    fn check_scw_output(
        &self,
        output: CommandOutput,
        resource: &str,
    ) -> Result<CommandOutput, JanitorError> {
        if output.is_success() {
            return Ok(output);
        }

        let status_text = output
            .code
            .map_or_else(|| String::from("unknown"), |code| code.to_string());
        Err(JanitorError::CommandFailure {
            program: self.config.scw_bin.clone(),
            status: output.code,
            status_text,
            stderr: format!("{resource}: {}", output.stderr),
        })
    }

    /// Lists resources using scw, returning parsed JSON.
    fn list_scw_resources<T>(
        &self,
        args: &[OsString],
        resource_name: &str,
    ) -> Result<Vec<T>, JanitorError>
    where
        T: serde::de::DeserializeOwned,
    {
        let stdout = self.run_scw_json(args, resource_name)?;
        serde_json::from_str::<Vec<T>>(&stdout).map_err(|err| JanitorError::Parse {
            resource: resource_name.to_owned(),
            message: err.to_string(),
        })
    }

    /// Builds argument vector for scw list commands.
    fn build_list_args(&self, subcommand_path: &[&str], filters: &[String]) -> Vec<OsString> {
        let mut args = Vec::new();

        // Subcommand path (e.g., ["instance", "server"])
        for part in subcommand_path {
            args.push(OsString::from(*part));
        }

        // Common list arguments
        args.push(OsString::from("list"));
        args.push(OsString::from(format!(
            "project-id={}",
            self.config.project_id
        )));
        args.push(OsString::from("zone=all"));

        // Additional filters
        for filter in filters {
            args.push(OsString::from(filter));
        }

        // JSON output format
        args.push(OsString::from("-o"));
        args.push(OsString::from("json"));

        args
    }

    fn run_scw_json(&self, args: &[OsString], resource: &str) -> Result<String, JanitorError> {
        let output = self.runner.run(&self.config.scw_bin, args)?;
        self.check_scw_output(output, resource)
            .map(|out| out.stdout)
    }

    fn run_scw(&self, args: &[OsString], resource: &str) -> Result<CommandOutput, JanitorError> {
        let output = self.runner.run(&self.config.scw_bin, args)?;
        self.check_scw_output(output, resource)
    }

    fn list_servers(&self) -> Result<Vec<ScwServer>, JanitorError> {
        let args = self.build_list_args(&["instance", "server"], &[String::from("name=mriya-")]);
        self.list_scw_resources(&args, "servers")
    }

    fn delete_server(&self, server: &ScwServer) -> Result<CommandOutput, JanitorError> {
        let args = vec![
            OsString::from("instance"),
            OsString::from("server"),
            OsString::from("delete"),
            OsString::from(&server.id),
            OsString::from(format!("zone={}", server.zone)),
            OsString::from("with-ip=true"),
            OsString::from("with-volumes=none"),
            OsString::from("force-shutdown=true"),
            OsString::from("--wait"),
        ];
        self.run_scw(&args, "server delete")
    }

    fn list_volumes(&self) -> Result<Vec<ScwVolume>, JanitorError> {
        let args = self.build_list_args(&["block", "volume"], &[]);
        self.list_scw_resources(&args, "volumes")
    }

    fn delete_volume(&self, volume: &ScwVolume) -> Result<CommandOutput, JanitorError> {
        let args = vec![
            OsString::from("block"),
            OsString::from("volume"),
            OsString::from("delete"),
            OsString::from(&volume.id),
            OsString::from(format!("zone={}", volume.zone)),
        ];
        self.run_scw(&args, "volume delete")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct ScwServer {
    id: String,
    zone: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct ScwVolume {
    id: String,
    zone: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[cfg(test)]
mod tests;
