//! Init request construction from loaded configuration.

use camino::Utf8PathBuf;
use mockable::{DefaultEnv, Env};

use super::{
    config::InitConfig,
    error::InitRequestError,
    helpers::{volume_name_for_project, volume_size_bytes},
};
use crate::{backend::InstanceRequest, config::ScalewayConfig, volume::VolumeRequest};

/// Environment variable that overrides the configured cache volume ID.
const VOLUME_ID_ENV_VAR: &str = "SCW_DEFAULT_VOLUME_ID";

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
    /// Volume ID supplied by `SCW_DEFAULT_VOLUME_ID`, tracked separately.
    ///
    /// The merged value cannot say which source supplied it, and an
    /// environment override keeps winning after the file is rewritten even
    /// when the file held the same old ID, so the warning needs this field.
    pub environment_volume_id: Option<String>,
}

impl InitRequest {
    /// Builds an init request from configuration and the request options.
    ///
    /// # Parameters
    ///
    /// - `scaleway`: credentials, zone, project and instance defaults.
    /// - `init_config`: size and tier settings for the new volume.
    /// - `options`: project name and whether `--force` was supplied.
    ///
    /// # Returns
    ///
    /// A request carrying the volume parameters, the formatter request, the
    /// effective merged volume ID and the environment-supplied ID.
    ///
    /// # Errors
    ///
    /// Returns [`InitRequestError`] when configuration or validation fails.
    pub fn from_config(
        scaleway: &ScalewayConfig,
        init_config: &InitConfig,
        options: InitOptions<'_>,
    ) -> Result<Self, InitRequestError> {
        Self::from_config_with_env(scaleway, init_config, options, &DefaultEnv)
    }

    /// Builds an init request, reading the environment override through `env`.
    ///
    /// # Parameters
    ///
    /// - `scaleway`: credentials, zone, project and instance defaults.
    /// - `init_config`: size and tier settings for the new volume.
    /// - `options`: project name and whether `--force` was supplied.
    /// - `env`: environment view used to detect `SCW_DEFAULT_VOLUME_ID`.
    ///
    /// # Returns
    ///
    /// A request carrying the volume parameters, the formatter request, the
    /// effective merged volume ID and the environment-supplied ID.
    ///
    /// # Errors
    ///
    /// Returns [`InitRequestError`] when configuration or validation fails.
    pub fn from_config_with_env(
        scaleway: &ScalewayConfig,
        init_config: &InitConfig,
        options: InitOptions<'_>,
        env: &impl Env,
    ) -> Result<Self, InitRequestError> {
        let InitOptions {
            project_name,
            overwrite_existing_volume_id,
        } = options;
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
            environment_volume_id: environment_volume_id(env),
        })
    }
}

/// Options that shape the generated init request.
#[derive(Clone, Copy, Debug)]
pub struct InitOptions<'a> {
    /// Directory name used to build the cache volume name.
    pub project_name: &'a str,
    /// Whether `--force` was supplied.
    pub overwrite_existing_volume_id: bool,
}

/// Returns the trimmed environment override, when the environment sets one.
fn environment_volume_id(env: &impl Env) -> Option<String> {
    env.raw(VOLUME_ID_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
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

#[cfg(test)]
mod tests {
    //! Unit tests for the environment override detection.
    use super::{VOLUME_ID_ENV_VAR, environment_volume_id};
    use crate::test_helpers::mock_env;

    #[test]
    fn an_environment_override_is_trimmed_and_kept() {
        let env = mock_env(&[(VOLUME_ID_ENV_VAR, "  vol-env  ")]);
        assert_eq!(environment_volume_id(&env).as_deref(), Some("vol-env"));
    }

    #[test]
    fn an_absent_or_blank_override_is_ignored() {
        assert_eq!(environment_volume_id(&mock_env(&[])), None);
        assert_eq!(
            environment_volume_id(&mock_env(&[(VOLUME_ID_ENV_VAR, "   ")])),
            None
        );
    }
}
