//! Scaleway test-resource janitor.
//!
//! The janitor is designed for integration tests that provision real cloud
//! resources. It identifies resources belonging to a specific test run via a
//! unique tag (`mriya-test-run-<id>`) and deletes them, failing if anything
//! remains afterwards.

use thiserror::Error;

use crate::sync::{CommandOutput, CommandRunner, ProcessCommandRunner, SyncError};

mod scw;
mod sweep;
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
    pub fn test_run_tag(&self) -> String { format!("{TEST_RUN_TAG_PREFIX}{}", self.test_run_id) }
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

/// Returns the zone to address a server's boot volume in.
///
/// An attached volume lives in the server's zone, so that zone is used when the
/// volume object omits its own.
pub(in crate::janitor) fn boot_volume_zone(
    volume: &types::ScwServerVolume,
    server: &ScwServer,
) -> String {
    if volume.zone.trim().is_empty() {
        return server.zone.clone();
    }
    volume.zone.clone()
}

/// Deletes test-run-tagged Scaleway resources by shelling out to `scw`.
#[derive(Clone, Debug)]
pub struct Janitor<R: CommandRunner> {
    /// Project, test-run tag, and `scw` binary to use.
    config: JanitorConfig,
    /// Executes the `scw` commands; replaceable so tests can script outputs.
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
    pub const fn new(config: JanitorConfig, runner: R) -> Self { Self { config, runner } }

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

        // The root volume of each tagged server carries no tag of its own, so
        // its identifier is recorded before the server is deleted. If the
        // server deletion removes the server but fails to remove the volume,
        // the retry below is the only thing that can find it again.
        let servers = self.list_tagged_servers(&tag)?;
        let boot_volumes = Self::boot_volume_refs(&servers);

        let mut first_error: Option<JanitorError> = None;
        for server in &servers {
            // A failed delete must not strand the root identifiers already
            // recorded: the sweep still tries every root below, and reports
            // what remains.
            if let Err(error) = self.delete_server(server) {
                first_error.get_or_insert(error);
            }
        }
        if let Err(error) = self.delete_boot_volumes(&boot_volumes, &mut first_error) {
            first_error.get_or_insert(error);
        }

        let deleted_volumes = self.delete_tagged_volumes(&tag)?;
        self.ensure_no_remaining(&tag, &boot_volumes)?;

        if let Some(error) = first_error {
            return Err(error);
        }

        Ok(SweepSummary {
            deleted_servers: servers.len(),
            deleted_volumes,
        })
    }

    /// Lists items with `list` and deletes each with `delete`, returning how many
    /// were found.
    ///
    /// The first failure aborts the sweep, leaving later items undeleted.
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

    /// Deletes every volume carrying `tag`.
    fn delete_tagged_volumes(&self, tag: &str) -> Result<usize, JanitorError> {
        self.delete_tagged(tag, Self::list_tagged_volumes, Self::delete_volume)
    }

    /// Re-lists tagged servers and volumes and fails with `NotClean` if any
    /// remain.
    ///
    /// The message names up to five of each as `id@zone`.
    fn ensure_no_remaining(
        &self,
        tag: &str,
        boot_volumes: &[(String, String, bool)],
    ) -> Result<(), JanitorError> {
        const MAX_ITEMS_TO_SHOW: usize = 5;

        let remaining_servers = self.list_tagged_servers(tag)?;
        let remaining_volumes = self.list_tagged_volumes(tag)?;
        let remaining_instance = self.list_instance_volumes()?;
        let remaining_block = self.list_block_volumes()?;
        let remaining_boot = boot_volumes
            .iter()
            .filter(|(id, _, is_block)| {
                let inventory = if *is_block {
                    &remaining_block
                } else {
                    &remaining_instance
                };
                inventory.iter().any(|volume| &volume.id == id)
            })
            .map(|(id, zone, _)| format!("{id}@{zone}"))
            .collect::<Vec<_>>();

        let no_tagged_resources = remaining_servers.is_empty() && remaining_volumes.is_empty();
        let nothing_remains = no_tagged_resources && remaining_boot.is_empty();
        if nothing_remains {
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
            "servers remaining: {} [{}], volumes remaining: {} [{}], boot volumes remaining: \
             {} [{}] (showing up to {} of each)",
            remaining_servers.len(),
            remaining_server_ids,
            remaining_volumes.len(),
            remaining_volume_ids,
            remaining_boot.len(),
            remaining_boot
                .iter()
                .take(MAX_ITEMS_TO_SHOW)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            MAX_ITEMS_TO_SHOW
        );
        Err(JanitorError::NotClean { message })
    }

    /// Reports whether `tags` contains `tag` exactly.
    fn has_tag(tags: &[String], tag: &str) -> bool { tags.iter().any(|existing| existing == tag) }

    /// Lists all servers in the project, keeping those carrying `tag`.
    fn list_tagged_servers(&self, tag: &str) -> Result<Vec<ScwServer>, JanitorError> {
        Ok(self
            .list_servers()?
            .into_iter()
            .filter(|srv| Self::has_tag(&srv.tags, tag))
            .collect())
    }

    /// Lists all volumes in the project, keeping those carrying `tag`.
    fn list_tagged_volumes(&self, tag: &str) -> Result<Vec<ScwVolume>, JanitorError> {
        Ok(self
            .list_volumes()?
            .into_iter()
            .filter(|vol| Self::has_tag(&vol.tags, tag))
            .collect())
    }
}

#[cfg(test)]
mod tests;
