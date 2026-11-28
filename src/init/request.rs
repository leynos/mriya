//! The request that describes a cache-volume preparation.
//!
//! Building the request catches an invalid configuration, and the
//! orchestrator's guard refuses a configured volume ID before `create_volume`.
//! A run that would replace a volume the user did not offer to replace
//! therefore stops before any billable resource is created, even though the
//! binary constructs its backend handle first.

use crate::{
    ScalewayConfig,
    init::{
        InitConfig,
        InitRequestError,
        InstanceRequest,
        VolumeRequest,
        volume_name_for_project,
        volume_size_bytes,
    },
};

/// Where the volume ID that `init` must respect comes from.
///
/// The orchestrator matches on this value instead of inferring a source from
/// two optional fields, so the environment case cannot be lost by a caller
/// forgetting to record it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum VolumeIdSource {
    /// No volume ID is configured.
    #[default]
    None,
    /// A usable ID in the configuration file the store edits or inherits.
    File,
    /// An ID supplied by the environment.
    ///
    /// An empty string records a present but blank variable, which the
    /// orchestrator refuses.
    Environment(String),
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
    /// Volume ID the resolved configuration supplied, if any.
    configured_volume_id: Option<String>,
    /// The source of the volume ID the run must respect.
    volume_id_source: VolumeIdSource,
}

impl InitRequest {
    /// Builds a request from its parts, with no configured volume ID.
    ///
    /// Prefer [`Self::from_config`] when configuration is available; this
    /// constructor exists for callers that already hold the request parts and
    /// supplies a [`VolumeIdSource::None`] source.
    #[must_use]
    pub const fn new(
        volume: VolumeRequest,
        instance_request: InstanceRequest,
        overwrite_existing_volume_id: bool,
    ) -> Self {
        Self {
            volume,
            instance_request,
            overwrite_existing_volume_id,
            configured_volume_id: None,
            volume_id_source: VolumeIdSource::None,
        }
    }

    /// Builds an init request from configuration and the project name.
    ///
    /// A usable volume ID from the resolved configuration is recorded on the
    /// request with a [`VolumeIdSource::File`] source. This constructor knows
    /// nothing about the environment; the composition module records
    /// [`VolumeIdSource::Environment`] when the environment supplies a value,
    /// and the orchestrator decides whether either source may be replaced.
    ///
    /// # Parameters
    ///
    /// - `scaleway`: the resolved Scaleway configuration.
    /// - `init_config`: the init-specific configuration.
    /// - `project_name`: the name used to derive the volume name.
    /// - `overwrite_existing_volume_id`: whether a configured ID may be replaced.
    ///
    /// # Returns
    ///
    /// A request whose configured ID is the usable resolved value, with a
    /// `File` source when one exists and `None` otherwise.
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
        let volume_id_source = if configured_volume_id.is_some() {
            VolumeIdSource::File
        } else {
            VolumeIdSource::None
        };
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
            volume_id_source,
        })
    }

    /// Records where the volume ID that the run must respect comes from.
    ///
    /// # Parameters
    ///
    /// - `source`: [`VolumeIdSource::Environment`] carries the environment value, with an empty
    ///   string for a present blank variable; `File` and `None` describe a configured or absent ID.
    ///
    /// # Returns
    ///
    /// The request with the source set, for chaining.
    #[must_use]
    pub fn with_volume_id_source(mut self, source: VolumeIdSource) -> Self {
        self.volume_id_source = source;
        self
    }

    /// Sets the volume-ID source on an existing request.
    pub fn set_volume_id_source(&mut self, source: VolumeIdSource) {
        self.volume_id_source = source;
    }

    /// The source of the volume ID the run must respect.
    #[must_use]
    pub const fn volume_id_source(&self) -> &VolumeIdSource { &self.volume_id_source }

    /// The usable ID the resolved configuration supplied, if any.
    #[must_use]
    pub fn configured_volume_id(&self) -> Option<&str> { self.configured_volume_id.as_deref() }
}

/// The volume ID the resolved configuration supplies, if it is usable.
///
/// A blank value records nothing and is ignored.
fn usable_volume_id(scaleway: &ScalewayConfig) -> Option<String> {
    scaleway
        .default_volume_id
        .as_deref()
        .map(str::trim)
        .filter(|volume_id| !volume_id.is_empty())
        .map(str::to_owned)
}
