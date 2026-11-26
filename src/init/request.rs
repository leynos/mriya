//! Init request construction from loaded configuration.

use camino::Utf8PathBuf;

use super::{
    config::InitConfig,
    error::InitRequestError,
    helpers::{volume_name_for_project, volume_size_bytes},
};
use crate::{backend::InstanceRequest, config::ScalewayConfig, volume::VolumeRequest};

/// Inputs required to prepare a cache volume.
#[derive(Clone, Debug)]
pub struct InitRequest {
    /// Volume creation parameters.
    pub volume: VolumeRequest,
    /// Instance request used to format the volume.
    pub instance_request: InstanceRequest,
    /// Whether to overwrite any existing configured volume ID.
    pub overwrite_existing_volume_id: bool,
    /// Effective merged volume ID at request time, including the environment.
    ///
    /// The file may hold a different value; this is what `mriya run` will
    /// actually use, so init must refuse to create a cache that would be
    /// ignored.
    pub effective_volume_id: Option<String>,
}

impl InitRequest {
    /// Builds an init request from configuration and the project name.
    ///
    /// `project_name` seeds the slug in the cache volume name, and
    /// `overwrite_existing_volume_id` is carried into the request so the
    /// orchestrator can refuse or replace an existing configured ID.
    ///
    /// # Parameters
    ///
    /// - `scaleway`: credentials, zone, project and instance defaults.
    /// - `init_config`: size and tier settings for the new volume.
    /// - `project_name`: directory name used to build the volume name.
    /// - `overwrite_existing_volume_id`: whether `--force` was supplied.
    ///
    /// # Returns
    ///
    /// A request carrying the volume parameters, the formatter request and the
    /// effective merged volume ID.
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
        let tier = init_config.tier()?;
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

        let effective_volume_id = scaleway
            .default_volume_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);

        let volume = VolumeRequest::new(
            volume_name,
            size_bytes,
            scaleway.default_zone.clone(),
            scaleway.default_project_id.clone(),
        )
        .organisation_id(scaleway.default_organization_id.clone())
        .with_tier(tier);

        Ok(Self {
            volume,
            instance_request,
            overwrite_existing_volume_id,
            effective_volume_id,
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
    /// Warning to show the user, such as an environment ID that will override
    /// the newly written file value.
    pub warning: Option<String>,
}
