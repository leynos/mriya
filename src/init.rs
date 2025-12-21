//! Volume initialisation orchestration for `mriya init`.

use std::ffi::OsString;
use std::fmt::Display;

use camino::Utf8PathBuf;
use ortho_config::OrthoConfig;
use serde::Deserialize;
use thiserror::Error;

use crate::backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest};
use crate::config::{ConfigError, ScalewayConfig};
use crate::config_store::{ConfigStoreError, ConfigWriter};
use crate::sync::{CommandRunner, RemoteCommandOutput, SyncError, Syncer};
use crate::volume::{VolumeBackend, VolumeHandle, VolumeRequest};

const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;
const FORMAT_COMMAND: &str = "sudo mkfs.ext4 -F /dev/vdb";

/// Init-specific configuration values layered via `OrthoConfig`.
#[derive(Clone, Debug, Deserialize, OrthoConfig, PartialEq, Eq)]
#[ortho_config(
    prefix = "MRIYA_INIT",
    discovery(
        app_name = "mriya",
        env_var = "MRIYA_CONFIG_PATH",
        config_file_name = "mriya.toml",
        dotfile_name = ".mriya.toml",
        project_file_name = "mriya.toml"
    )
)]
pub struct InitConfig {
    /// Size of the cache volume in gigabytes.
    #[ortho_config(default = 20)]
    pub volume_size_gb: u32,
}

impl InitConfig {
    /// Loads init configuration without parsing CLI arguments.
    ///
    /// # Errors
    ///
    /// Returns [`InitConfigError::Parse`] when merging sources fails.
    pub fn load_without_cli_args() -> Result<Self, InitConfigError> {
        Self::load_from_iter([OsString::from("mriya")])
            .map_err(|err| InitConfigError::Parse(err.to_string()))
    }

    /// Validates init configuration.
    ///
    /// # Errors
    ///
    /// Returns [`InitConfigError::InvalidVolumeSize`] when the configured
    /// volume size is zero.
    pub const fn validate(&self) -> Result<(), InitConfigError> {
        if self.volume_size_gb == 0 {
            return Err(InitConfigError::InvalidVolumeSize);
        }
        Ok(())
    }
}

/// Errors raised while loading init configuration.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum InitConfigError {
    /// Raised when configuration parsing fails.
    #[error("init configuration parsing failed: {0}")]
    Parse(String),
    /// Raised when volume size is invalid.
    #[error("volume size must be greater than zero")]
    InvalidVolumeSize,
}

/// Inputs required to prepare a cache volume.
#[derive(Clone, Debug)]
pub struct InitRequest {
    /// Volume creation parameters.
    pub volume: VolumeRequest,
    /// Instance request used to format the volume.
    pub instance_request: InstanceRequest,
    /// Whether to overwrite any existing configured volume ID.
    pub overwrite_existing_volume_id: bool,
}

impl InitRequest {
    /// Builds an init request from configuration and the project name.
    ///
    /// # Errors
    ///
    /// Returns [`InitRequestError`] when configuration or validation fails.
    pub fn from_config(
        scaleway: &ScalewayConfig,
        init_config: &InitConfig,
        project_name: &str,
        overwrite_existing_volume_id: bool,
    ) -> Result<Self, InitRequestError> {
        init_config.validate()?;
        let volume_name = volume_name_for_project(project_name);
        if volume_name.trim().is_empty() {
            return Err(InitRequestError::InvalidVolumeName);
        }
        let size_bytes =
            volume_size_bytes(init_config.volume_size_gb).ok_or(InitRequestError::SizeOverflow)?;

        scaleway.validate()?;
        let instance_request = InstanceRequest::builder()
            .image_label(&scaleway.default_image)
            .instance_type(&scaleway.default_instance_type)
            .zone(&scaleway.default_zone)
            .project_id(&scaleway.default_project_id)
            .organisation_id(scaleway.default_organization_id.clone())
            .architecture(&scaleway.default_architecture)
            .cloud_init_user_data(None)
            .build()
            .map_err(|err| InitRequestError::RequestBuild(err.to_string()))?;

        let volume = VolumeRequest::new(
            volume_name,
            size_bytes,
            scaleway.default_zone.clone(),
            scaleway.default_project_id.clone(),
        )
        .organisation_id(scaleway.default_organization_id.clone());

        Ok(Self {
            volume,
            instance_request,
            overwrite_existing_volume_id,
        })
    }
}

/// Errors raised while preparing an init request.
#[derive(Debug, Error)]
pub enum InitRequestError {
    /// Raised when Scaleway configuration is invalid.
    #[error("scaleway configuration error: {0}")]
    Config(#[from] ConfigError),
    /// Raised when init configuration is invalid.
    #[error("init configuration error: {0}")]
    InitConfig(#[from] InitConfigError),
    /// Raised when building the formatter instance request fails.
    #[error("instance request error: {0}")]
    RequestBuild(String),
    /// Raised when the computed volume name is invalid.
    #[error("volume name must not be empty")]
    InvalidVolumeName,
    /// Raised when the volume size cannot be represented in bytes.
    #[error("volume size is too large to represent")]
    SizeOverflow,
}

/// Outcome returned after successfully preparing a cache volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitOutcome {
    /// Identifier of the newly created volume.
    pub volume_id: String,
    /// Configuration file path that was updated.
    pub config_path: Utf8PathBuf,
}

/// Errors raised while initialising a cache volume.
#[derive(Debug, Error)]
pub enum InitError<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    /// Raised when configuration updates fail.
    #[error("configuration update failed: {0}")]
    Config(#[from] ConfigStoreError),
    /// Raised when volume creation fails.
    #[error("failed to create volume: {0}")]
    Volume(#[source] BackendError),
    /// Raised when instance creation fails.
    #[error("failed to provision formatter instance: {0}")]
    Provision(#[source] BackendError),
    /// Raised when instance readiness checks fail.
    #[error("instance did not become ready: {message}")]
    Wait {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: BackendError,
    },
    /// Raised when formatting fails.
    #[error("volume format failed: {message}")]
    Format {
        /// Human-readable description of the failure.
        message: String,
        /// Underlying sync error, if any.
        #[source]
        source: Option<SyncError>,
    },
    /// Raised when volume detachment fails.
    #[error("failed to detach volume: {message}")]
    Detach {
        /// Human-readable description of the failure.
        message: String,
        /// Provider-specific error.
        #[source]
        source: BackendError,
    },
    /// Raised when teardown fails after formatting succeeds.
    #[error("failed to destroy formatter instance: {0}")]
    Teardown(#[source] BackendError),
}

/// Coordinates volume creation, formatting, and configuration updates.
#[derive(Debug)]
pub struct InitOrchestrator<B, R: CommandRunner, W> {
    backend: B,
    syncer: Syncer<R>,
    config_writer: W,
}

impl<B, R, W> InitOrchestrator<B, R, W>
where
    B: Backend + VolumeBackend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
    W: ConfigWriter,
{
    /// Creates a new init orchestrator.
    #[must_use]
    pub const fn new(backend: B, syncer: Syncer<R>, config_writer: W) -> Self {
        Self {
            backend,
            syncer,
            config_writer,
        }
    }

    /// Executes the cache volume preparation workflow.
    ///
    /// # Errors
    ///
    /// Returns [`InitError`] when volume creation, formatting, teardown, or
    /// configuration updates fail.
    pub async fn execute(&self, request: &InitRequest) -> Result<InitOutcome, InitError<B::Error>> {
        self.ensure_configurable(request)?;

        let volume = self
            .backend
            .create_volume(&request.volume)
            .await
            .map_err(InitError::Volume)?;

        let (handle, networking) = self.prepare_instance(request, &volume).await?;
        self.format_or_destroy(&handle, &networking).await?;
        self.detach_or_destroy(&handle, &volume.id).await?;

        self.backend
            .destroy(handle)
            .await
            .map_err(InitError::Teardown)?;

        let config_path = self
            .config_writer
            .write_volume_id(&volume.id, request.overwrite_existing_volume_id)?;

        Ok(InitOutcome {
            volume_id: volume.id,
            config_path,
        })
    }

    fn ensure_configurable(&self, request: &InitRequest) -> Result<(), InitError<B::Error>> {
        if let Some(existing) = self.config_writer.current_volume_id()?
            && !request.overwrite_existing_volume_id
        {
            return Err(InitError::Config(
                ConfigStoreError::VolumeAlreadyConfigured {
                    volume_id: existing,
                },
            ));
        }
        Ok(())
    }

    async fn prepare_instance(
        &self,
        request: &InitRequest,
        volume: &VolumeHandle,
    ) -> Result<(InstanceHandle, InstanceNetworking), InitError<B::Error>> {
        let mut instance_request = request.instance_request.clone();
        instance_request.volume_id = Some(volume.id.clone());
        instance_request.cloud_init_user_data = None;

        let handle = self
            .backend
            .create(&instance_request)
            .await
            .map_err(InitError::Provision)?;

        let networking = self.wait_for_ready_or_destroy(&handle).await?;
        Ok((handle, networking))
    }

    async fn wait_for_ready_or_destroy(
        &self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, InitError<B::Error>> {
        match self.backend.wait_for_ready(handle).await {
            Ok(net) => Ok(net),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(InitError::Wait {
                    message,
                    source: err,
                })
            }
        }
    }

    async fn handle_failure_or_destroy<E>(
        &self,
        handle: &InstanceHandle,
        result: Result<(), E>,
        make_error: impl FnOnce(String, E) -> InitError<B::Error>,
    ) -> Result<(), InitError<B::Error>>
    where
        E: Display,
    {
        match result {
            Ok(()) => Ok(()),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(make_error(message, err))
            }
        }
    }

    async fn format_or_destroy(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
    ) -> Result<(), InitError<B::Error>> {
        let result = self.format_volume(networking);
        self.handle_failure_or_destroy(handle, result, |message, failure| InitError::Format {
            message,
            source: failure.source,
        })
        .await
    }

    async fn detach_or_destroy(
        &self,
        handle: &InstanceHandle,
        volume_id: &str,
    ) -> Result<(), InitError<B::Error>> {
        let result = self.backend.detach_volume(handle, volume_id).await;
        self.handle_failure_or_destroy(handle, result, |message, err| InitError::Detach {
            message,
            source: err,
        })
        .await
    }

    fn format_volume(&self, networking: &InstanceNetworking) -> Result<(), FormatFailure> {
        let output = self
            .syncer
            .run_remote_raw(networking, FORMAT_COMMAND)
            .map_err(|err| FormatFailure {
                message: String::from("failed to execute format command"),
                source: Some(err),
            })?;

        if output.exit_code == Some(0) {
            return Ok(());
        }

        Err(FormatFailure {
            message: format_failure_message(&output),
            source: None,
        })
    }

    async fn destroy_with_note<E: Display>(&self, handle: &InstanceHandle, err: &E) -> String {
        let teardown_error = self.backend.destroy(handle.clone()).await.err();
        append_teardown_note(err.to_string(), teardown_error.as_ref())
    }
}

#[derive(Debug)]
struct FormatFailure {
    message: String,
    source: Option<SyncError>,
}

impl Display for FormatFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

fn format_failure_message(output: &RemoteCommandOutput) -> String {
    let stderr = output.stderr.trim();
    match output.exit_code {
        Some(code) if stderr.is_empty() => format!("mkfs.ext4 exited with status {code}"),
        Some(code) => format!("mkfs.ext4 exited with status {code}: {stderr}"),
        None if stderr.is_empty() => String::from("mkfs.ext4 terminated without an exit status"),
        None => format!("mkfs.ext4 terminated without an exit status: {stderr}"),
    }
}

fn append_teardown_note<E: Display>(message: String, teardown_error: Option<&E>) -> String {
    if let Some(teardown) = teardown_error {
        format!("{message} (teardown also failed: {teardown})")
    } else {
        message
    }
}

fn volume_size_bytes(size_gb: u32) -> Option<u64> {
    u64::from(size_gb).checked_mul(BYTES_PER_GB)
}

fn volume_name_for_project(project_name: &str) -> String {
    let slug = slugify(project_name);
    if slug.is_empty() {
        return String::from("mriya-cache");
    }
    format!("mriya-{slug}-cache")
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_name_for_project_defaults_when_empty() {
        let name = volume_name_for_project("");
        assert_eq!(name, "mriya-cache");
    }

    #[test]
    fn volume_name_for_project_slugifies() {
        let name = volume_name_for_project("Fancy Project!");
        assert_eq!(name, "mriya-fancy-project-cache");
    }

    #[test]
    fn volume_size_bytes_converts_gb() {
        let bytes = volume_size_bytes(2).unwrap_or_else(|| panic!("size bytes"));
        assert_eq!(bytes, 2 * BYTES_PER_GB);
    }
}
