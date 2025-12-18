//! Scaleway test-resource janitor.
//!
//! The janitor is designed for integration tests that provision real cloud
//! resources. It identifies resources belonging to a specific test run via a
//! unique tag (`mriya-test-run-<id>`) and deletes them, failing if anything
//! remains afterwards.

use std::ffi::OsString;

use serde_json::Value;
use thiserror::Error;

use crate::sync::{CommandOutput, CommandRunner, ProcessCommandRunner, SyncError};

pub mod types;
pub use types::{ScwServer, ScwVolume};

/// Environment variable used by test harnesses to identify a test run.
pub const TEST_RUN_ID_ENV: &str = "MRIYA_TEST_RUN_ID";

/// Prefix used for test run tags applied to Scaleway resources.
pub const TEST_RUN_TAG_PREFIX: &str = "mriya-test-run-";

/// Default Scaleway CLI binary name.
pub const DEFAULT_SCW_BIN: &str = "scw";

/// Scaleway resource type for janitor operations.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ResourceType {
    /// Instance servers.
    Servers,
    /// Block storage volumes.
    Volumes,
}

impl ResourceType {
    /// Returns the resource name used in scw CLI commands and JSON parsing.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Servers => "servers",
            Self::Volumes => "volumes",
        }
    }
}

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
    /// Trims a required field and rejects blank values.
    ///
    /// # Errors
    ///
    /// Returns [`JanitorError::InvalidConfig`] when any required field is blank.
    fn require_non_blank(value: &str, field: &'static str) -> Result<String, JanitorError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(JanitorError::InvalidConfig {
                field: field.to_owned(),
            });
        }
        Ok(trimmed.to_owned())
    }

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
        let raw_project_id = project_id.into();
        let raw_test_run_id = test_run_id.into();
        let raw_scw_bin = scw_bin.into();

        let trimmed_project_id = Self::require_non_blank(&raw_project_id, "project_id")?;
        let trimmed_test_run_id = Self::require_non_blank(&raw_test_run_id, "test_run_id")?;
        let trimmed_scw_bin = Self::require_non_blank(&raw_scw_bin, "scw_bin")?;
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

        let deleted_servers = self.delete_tagged_servers(&tag)?;
        let deleted_volumes = self.delete_tagged_volumes(&tag)?;
        self.ensure_no_remaining(&tag)?;

        Ok(SweepSummary {
            deleted_servers,
            deleted_volumes,
        })
    }

    fn delete_tagged<T>(
        &self,
        tag: &str,
        list: fn(&Self, &str) -> Result<Vec<T>, JanitorError>,
        delete: fn(&Self, &T) -> Result<CommandOutput, JanitorError>,
    ) -> Result<usize, JanitorError> {
        let items = list(self, tag)?;
        for item in &items {
            delete(self, item)?;
        }
        Ok(items.len())
    }

    fn delete_tagged_servers(&self, tag: &str) -> Result<usize, JanitorError> {
        self.delete_tagged(tag, Self::list_tagged_servers, Self::delete_server)
    }

    fn delete_tagged_volumes(&self, tag: &str) -> Result<usize, JanitorError> {
        self.delete_tagged(tag, Self::list_tagged_volumes, Self::delete_volume)
    }

    fn ensure_no_remaining(&self, tag: &str) -> Result<(), JanitorError> {
        const MAX_ITEMS_TO_SHOW: usize = 5;

        let remaining_servers = self.list_tagged_servers(tag)?;
        let remaining_volumes = self.list_tagged_volumes(tag)?;

        if remaining_servers.is_empty() && remaining_volumes.is_empty() {
            return Ok(());
        }

        let remaining_server_ids = remaining_servers
            .iter()
            .take(MAX_ITEMS_TO_SHOW)
            .map(|srv| format!("{}@{}", srv.id, srv.zone))
            .collect::<Vec<_>>()
            .join(", ");
        let remaining_volume_ids = remaining_volumes
            .iter()
            .take(MAX_ITEMS_TO_SHOW)
            .map(|vol| format!("{}@{}", vol.id, vol.zone))
            .collect::<Vec<_>>()
            .join(", ");

        let message = format!(
            "servers remaining: {} [{}], volumes remaining: {} [{}] (showing up to {} of each)",
            remaining_servers.len(),
            remaining_server_ids,
            remaining_volumes.len(),
            remaining_volume_ids,
            MAX_ITEMS_TO_SHOW
        );
        Err(JanitorError::NotClean { message })
    }

    fn has_tag(tags: &[String], tag: &str) -> bool {
        tags.iter().any(|existing| existing == tag)
    }

    fn list_tagged_servers(&self, tag: &str) -> Result<Vec<ScwServer>, JanitorError> {
        Ok(self
            .list_servers()?
            .into_iter()
            .filter(|srv| Self::has_tag(&srv.tags, tag))
            .collect())
    }

    fn list_tagged_volumes(&self, tag: &str) -> Result<Vec<ScwVolume>, JanitorError> {
        Ok(self
            .list_volumes()?
            .into_iter()
            .filter(|vol| Self::has_tag(&vol.tags, tag))
            .collect())
    }

    fn run_scw(
        &self,
        args: &[&str],
        resource: ResourceType,
    ) -> Result<CommandOutput, JanitorError> {
        let os_args = args.iter().map(OsString::from).collect::<Vec<_>>();
        let output = self.runner.run(&self.config.scw_bin, &os_args)?;

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
            stderr: format!("{}: {}", resource.as_str(), output.stderr),
        })
    }

    fn run_scw_json(&self, args: &[&str], resource: ResourceType) -> Result<String, JanitorError> {
        self.run_scw(args, resource).map(|out| out.stdout)
    }

    fn parse_scw_list<T>(stdout: &str, resource: ResourceType) -> Result<Vec<T>, JanitorError>
    where
        T: serde::de::DeserializeOwned,
    {
        let resource_name = resource.as_str();
        let payload = serde_json::from_str::<Value>(stdout).map_err(|err| JanitorError::Parse {
            resource: resource_name.to_owned(),
            message: err.to_string(),
        })?;

        let items = Self::unwrap_scw_list_items(payload, resource_name)?;

        serde_json::from_value::<Vec<T>>(items).map_err(|err| JanitorError::Parse {
            resource: resource_name.to_owned(),
            message: err.to_string(),
        })
    }

    /// Lists Scaleway resources of a specific type using the scw CLI.
    fn list_scw_resources<T>(
        &self,
        subcommand_path: &[&str],
        resource: ResourceType,
    ) -> Result<Vec<T>, JanitorError>
    where
        T: serde::de::DeserializeOwned,
    {
        let project_arg = format!("project-id={}", self.config.project_id);

        // Capacity: `subcommand_path` + ["list", project_arg, "zone=all", "-o", "json"].
        let mut args = Vec::with_capacity(subcommand_path.len() + 5);
        args.extend_from_slice(subcommand_path);
        args.extend_from_slice(&["list", project_arg.as_str(), "zone=all", "-o", "json"]);

        let stdout = self.run_scw_json(&args, resource)?;
        Self::parse_scw_list(&stdout, resource)
    }

    fn unwrap_scw_list_items(payload: Value, resource_name: &str) -> Result<Value, JanitorError> {
        match payload {
            Value::Array(items) => Ok(Value::Array(items)),
            Value::Object(map) => Self::extract_scw_list_field(map, resource_name),
            other => Err(JanitorError::Parse {
                resource: resource_name.to_owned(),
                message: format!("unexpected JSON shape: {other}"),
            }),
        }
    }

    fn extract_scw_list_field(
        mut map: serde_json::Map<String, Value>,
        resource_name: &str,
    ) -> Result<Value, JanitorError> {
        map.remove(resource_name)
            .ok_or_else(|| JanitorError::Parse {
                resource: resource_name.to_owned(),
                message: format!("missing '{resource_name}' field"),
            })
    }

    fn list_servers(&self) -> Result<Vec<ScwServer>, JanitorError> {
        self.list_scw_resources(&["instance", "server"], ResourceType::Servers)
    }

    fn delete_server(&self, server: &ScwServer) -> Result<CommandOutput, JanitorError> {
        let zone_arg = format!("zone={}", server.zone);
        let args = [
            "instance",
            "server",
            "delete",
            server.id.as_str(),
            zone_arg.as_str(),
            "with-ip=true",
            "with-volumes=none",
            "force-shutdown=true",
            "--wait",
        ];
        self.run_scw(&args, ResourceType::Servers)
    }

    fn list_volumes(&self) -> Result<Vec<ScwVolume>, JanitorError> {
        self.list_scw_resources(&["block", "volume"], ResourceType::Volumes)
    }

    fn delete_volume(&self, volume: &ScwVolume) -> Result<CommandOutput, JanitorError> {
        let zone_arg = format!("zone={}", volume.zone);
        let args = [
            "block",
            "volume",
            "delete",
            volume.id.as_str(),
            zone_arg.as_str(),
        ];
        self.run_scw(&args, ResourceType::Volumes)
    }
}

#[cfg(test)]
mod tests;
