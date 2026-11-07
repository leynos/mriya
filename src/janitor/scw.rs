//! Scaleway CLI invocation, JSON list parsing, and resource deletion for the
//! janitor.

use std::ffi::OsString;

use serde_json::Value;

use super::{Janitor, JanitorError, ResourceType, ScwServer, ScwVolume};
use crate::sync::{CommandOutput, CommandRunner};

impl<R: CommandRunner> Janitor<R> {
    /// Runs `scw` with `args`, failing when it exits unsuccessfully.
    ///
    /// The failure's `stderr` is prefixed with the resource name for context.
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

    /// Runs `scw` and returns its standard output, which the caller parses as
    /// JSON.
    fn run_scw_json(&self, args: &[&str], resource: ResourceType) -> Result<String, JanitorError> {
        self.run_scw(args, resource).map(|out| out.stdout)
    }

    /// Parses `scw` list output into items of type `T`.
    ///
    /// Both a bare JSON array and an object wrapping the array under the
    /// resource's name are accepted.
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

    /// Returns the array of items from either list output shape.
    ///
    /// # Errors
    ///
    /// Returns `Parse` for anything other than an array or object.
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

    /// Takes the array stored under `resource_name` from a wrapper object.
    ///
    /// # Errors
    ///
    /// Returns `Parse` when the field is missing.
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

    /// Lists servers in the project across all zones.
    pub(super) fn list_servers(&self) -> Result<Vec<ScwServer>, JanitorError> {
        self.list_scw_resources(&["instance", "server"], ResourceType::Servers)
    }

    /// Deletes a server, releasing its IP and force-shutting it down, and waits
    /// for completion.
    ///
    /// Attached volumes are kept (`with-volumes=none`) and handled separately.
    pub(super) fn delete_server(&self, server: &ScwServer) -> Result<CommandOutput, JanitorError> {
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

    /// Lists Block Storage volumes in the project across all zones.
    pub(super) fn list_volumes(&self) -> Result<Vec<ScwVolume>, JanitorError> {
        self.list_scw_resources(&["block", "volume"], ResourceType::Volumes)
    }

    /// Deletes a Block Storage volume in its own zone.
    pub(super) fn delete_volume(&self, volume: &ScwVolume) -> Result<CommandOutput, JanitorError> {
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
