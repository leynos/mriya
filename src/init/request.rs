//! The request that describes a cache-volume preparation.
//!
//! The request is built from configuration before any backend exists, so the
//! checks here can refuse a run that would create a billable volume the user
//! does not want replaced.

use crate::{
    ScalewayConfig,
    config_store::ConfigStoreError,
    init::{
        InitConfig,
        InitRequestError,
        InstanceRequest,
        VolumeRequest,
        volume_name_for_project,
        volume_size_bytes,
    },
};

/// Inputs required to prepare a cache volume.
#[derive(Clone, Debug)]
pub struct InitRequest {
    /// Volume creation parameters.
    pub volume: VolumeRequest,
    /// Instance request used to format the volume.
    pub instance_request: InstanceRequest,
    /// Whether to overwrite any existing configured volume ID.
    pub overwrite_existing_volume_id: bool,
    /// Volume ID the resolved configuration supplied, if any.
    ///
    /// Used to tell a file-backed ID, which a forced run may replace, from one
    /// supplied by the environment, which a file write cannot replace.
    pub configured_volume_id: Option<String>,
}

impl InitRequest {
    /// Builds an init request from configuration and the project name.
    ///
    /// A usable volume ID from the resolved configuration counts as existing
    /// configuration: without `--force` the request fails rather than
    /// replacing it.
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
        let configured_volume_id = usable_volume_id(scaleway);
        if !overwrite_existing_volume_id && let Some(volume_id) = configured_volume_id.as_deref() {
            return Err(InitRequestError::Store(
                ConfigStoreError::VolumeAlreadyConfigured {
                    volume_id: volume_id.to_owned(),
                },
            ));
        }
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
            configured_volume_id,
        })
    }
}

/// The volume ID the resolved configuration supplies, if it is usable.
///
/// The effective configuration can carry an ID from an environment variable as
/// well as from the file, and `run` prefers that value over anything `init`
/// writes. A blank value records nothing and is ignored.
fn usable_volume_id(scaleway: &ScalewayConfig) -> Option<String> {
    scaleway
        .default_volume_id
        .as_deref()
        .map(str::trim)
        .filter(|volume_id| !volume_id.is_empty())
        .map(str::to_owned)
}
