//! The request that describes a cache-volume preparation.
//!
//! Building the request catches an invalid configuration, and the
//! orchestrator's guard refuses a configured volume ID before `create_volume`.
//! A run that would replace a volume the user did not offer to replace
//! therefore stops before any billable resource is created, even though the
//! binary constructs its backend handle first.

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
    /// Used to refuse an unforced replacement, whichever source supplied it.
    pub configured_volume_id: Option<String>,
    /// The volume ID the environment supplies, when the caller can tell.
    ///
    /// A file write cannot change what the environment provides, so `init`
    /// refuses even a forced replacement while this is set.
    pub environment_volume_id: Option<String>,
}

/// Environment variable that supplies the volume ID directly.
pub const VOLUME_ID_ENV_VAR: &str = "SCW_DEFAULT_VOLUME_ID";

/// Reads the volume-ID variable through an injected environment boundary.
///
/// The key match is case-insensitive, because the loader's environment
/// provider matches keys that way; a guard that looked only for the canonical
/// spelling would miss a value the loader still applies. Returns `None` when no
/// spelling is present and the trimmed value when it is usable. A present blank
/// value is returned as an empty string so the request carries the provenance;
/// the loader still merges it over the file, and the orchestrator refuses it
/// before any backend work.
///
/// # Errors
///
/// Returns [`InitRequestError::NonUtf8EnvironmentVolumeId`] for a value the
/// boundary cannot decode. A present blank value is returned as an empty
/// string so the request carries the provenance; the orchestrator rejects it.
pub fn environment_volume_id(
    env: &dyn ortho_config::ScanEnvSource,
) -> Result<Option<String>, InitRequestError> {
    let mut found = None;
    for (key, raw) in env.scan() {
        if !key
            .to_string_lossy()
            .eq_ignore_ascii_case(VOLUME_ID_ENV_VAR)
        {
            continue;
        }
        let text = raw
            .into_string()
            .map_err(|_| InitRequestError::NonUtf8EnvironmentVolumeId {
                variable: VOLUME_ID_ENV_VAR,
            })?;
        let trimmed = text.trim();
        // A blank layer is still a layer: record it as present so the
        // orchestrator refuses before any backend work rather than treating
        // the variable as absent.
        found = Some(trimmed.to_owned());
    }
    Ok(found)
}

/// The loaded configuration an init request is built from.
#[derive(Clone, Copy)]
pub struct InitRequestInputs<'a> {
    /// Scaleway configuration resolved from defaults, files, and environment.
    pub scaleway: &'a ScalewayConfig,
    /// Init-specific configuration.
    pub init_config: &'a InitConfig,
    /// Project name used for the volume name.
    pub project_name: &'a str,
    /// Whether a configured volume ID may be replaced.
    pub overwrite_existing_volume_id: bool,
}

/// Builds the init request the binary would use for a loaded configuration.
///
/// This is the composition root's whole decision: the request is built from
/// configuration, and the environment's own volume ID is recorded so a forced
/// run cannot silently replace a value only the environment controls.
///
/// # Errors
///
/// Returns [`InitRequestError`] when the configuration is invalid or the
/// environment supplies an unusable volume ID.
pub fn request_from_loaded_config(
    inputs: InitRequestInputs<'_>,
    env: &dyn ortho_config::ScanEnvSource,
) -> Result<InitRequest, InitRequestError> {
    let environment_volume_id = environment_volume_id(env)?;
    InitRequest::from_config(
        inputs.scaleway,
        inputs.init_config,
        inputs.project_name,
        inputs.overwrite_existing_volume_id,
    )
    .map(|request| request.with_environment_volume_id(environment_volume_id))
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
            environment_volume_id: None,
        })
    }

    /// Records the volume ID the environment supplies, if the caller can tell.
    ///
    /// The composition root reads `SCW_DEFAULT_VOLUME_ID` because only it has
    /// the process environment; the orchestrator then refuses a forced
    /// replacement even when the file holds the same value.
    #[must_use]
    pub fn with_environment_volume_id(mut self, volume_id: Option<String>) -> Self {
        self.environment_volume_id = volume_id;
        self
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
