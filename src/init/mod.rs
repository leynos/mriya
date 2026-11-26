//! Volume initialization orchestration for `mriya init`.
//!
//! Initialization creates a volume, formats it from a helper instance, and
//! records its identifier in configuration. The new volume is owned by the run
//! until formatting succeeds, when it is promoted to retained, and every exit
//! runs ownership-aware cleanup.
//!
//! What cleanup achieves is a verified outcome, not a guarantee: a failure
//! before formatting normally deletes the volume, but a provider error, a
//! permission denial, or a second signal can leave it attached or present. A
//! failure after formatting detaches the volume and keeps it for the operator.
//! Both cases report the volume's identifier and, when resources may remain,
//! the journal that `mriya recover` replays.

use std::ffi::OsString;

use camino::Utf8PathBuf;
use helpers::{volume_name_for_project, volume_size_bytes};
use ortho_config::OrthoConfig;
use serde::Deserialize;

use crate::{backend::InstanceRequest, config::ScalewayConfig, volume::VolumeRequest};

mod error;
mod finish;
mod format;
mod helpers;
mod orchestrator;
mod outcome;

pub use error::{InitConfigError, InitError, InitErrorKind, InitRequestError};
pub use orchestrator::InitOrchestrator;

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

/// Outcome returned after successfully preparing a cache volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitOutcome {
    /// Identifier of the newly created volume.
    pub volume_id: String,
    /// Configuration file path that was updated.
    pub config_path: Utf8PathBuf,
    /// Non-fatal notes gathered during cleanup, such as a journal that could
    /// not be rewritten.
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests;
