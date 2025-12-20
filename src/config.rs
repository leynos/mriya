//! Configuration loading via `ortho-config`.

use crate::backend::InstanceRequest;
use crate::cloud_init::{CloudInitError, resolve_cloud_init_user_data};
use ortho_config::OrthoConfig;
use serde::Deserialize;
use thiserror::Error;

/// TOML section name for Scaleway configuration.
const SCALEWAY_SECTION: &str = "scaleway";

/// Scaleway specific configuration derived from environment variables,
/// configuration files, and CLI flags.
#[derive(Clone, Debug, Deserialize, OrthoConfig, PartialEq, Eq)]
#[ortho_config(prefix = "SCW")]
pub struct ScalewayConfig {
    /// Access key assigned to the Scaleway application. While not required for
    /// API calls, it is captured to support future audit logging.
    pub access_key: Option<String>,
    /// Secret key used for authentication. This value is required.
    pub secret_key: String,
    /// Organisation identifier used by some Scaleway endpoints.
    pub default_organization_id: Option<String>,
    /// Project identifier used for billing and resource scoping.
    pub default_project_id: String,
    /// Preferred availability zone. Defaults to `fr-par-1`.
    #[ortho_config(default = "fr-par-1".to_owned())]
    pub default_zone: String,
    /// Commercial type for new instances. Defaults to `DEV1-S` to minimise
    /// cost during integration tests.
    #[ortho_config(default = "DEV1-S".to_owned())]
    pub default_instance_type: String,
    /// Human-friendly image label (for example `Ubuntu 24.04 Noble Numbat`).
    #[ortho_config(default = "Ubuntu 24.04 Noble Numbat".to_owned())]
    pub default_image: String,
    /// CPU architecture used to select the correct image variant.
    #[ortho_config(default = "x86_64".to_owned())]
    pub default_architecture: String,
    /// Optional Block Storage volume ID to attach for persistent caching.
    /// The volume must exist in the same zone as the instance.
    pub default_volume_id: Option<String>,
    /// Optional cloud-init user-data payload (cloud-config YAML or script).
    pub cloud_init_user_data: Option<String>,
    /// Optional path to a file containing cloud-init user-data.
    pub cloud_init_user_data_file: Option<String>,
}

/// Metadata for a configuration field, used to generate actionable error messages.
struct FieldMetadata {
    description: &'static str,
    env_var: &'static str,
    toml_key: &'static str,
    section: &'static str,
}

impl FieldMetadata {
    const fn new(
        description: &'static str,
        env_var: &'static str,
        toml_key: &'static str,
        section: &'static str,
    ) -> Self {
        Self {
            description,
            env_var,
            toml_key,
            section,
        }
    }
}

impl ScalewayConfig {
    fn require_field(value: &str, metadata: &FieldMetadata) -> Result<(), ConfigError> {
        if value.trim().is_empty() {
            return Err(ConfigError::MissingField(format!(
                "missing {}: set {} or add {} to [{}] in mriya.toml",
                metadata.description, metadata.env_var, metadata.toml_key, metadata.section
            )));
        }
        Ok(())
    }

    /// Loads configuration using the `ortho-config` derive. Values merge
    /// defaults, configuration files, environment variables, and CLI flags in
    /// that order of precedence.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] when the loader fails to merge sources.
    pub fn load_from_sources() -> Result<Self, ConfigError> {
        Self::load().map_err(|err| ConfigError::Parse(err.to_string()))
    }

    /// Loads configuration without attempting to parse CLI arguments. Values
    /// still merge defaults, configuration files, and environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] when the merge fails.
    pub fn load_without_cli_args() -> Result<Self, ConfigError> {
        Self::load_from_iter([std::ffi::OsString::from("mriya")])
            .map_err(|err| ConfigError::Parse(err.to_string()))
    }

    /// Builds an [`InstanceRequest`] using the configured defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when validation fails.
    pub fn as_request(&self) -> Result<InstanceRequest, ConfigError> {
        self.validate_required_fields()?;
        let cloud_init_user_data = self.resolve_cloud_init_user_data()?;
        InstanceRequest::builder()
            .image_label(&self.default_image)
            .instance_type(&self.default_instance_type)
            .zone(&self.default_zone)
            .project_id(&self.default_project_id)
            .organisation_id(self.default_organization_id.clone())
            .architecture(&self.default_architecture)
            .volume_id(self.default_volume_id.clone())
            .cloud_init_user_data(cloud_init_user_data)
            .build()
            .map_err(|err| ConfigError::Parse(err.to_string()))
    }

    fn resolve_cloud_init_user_data(&self) -> Result<Option<String>, ConfigError> {
        resolve_cloud_init_user_data(
            self.cloud_init_user_data.as_deref(),
            self.cloud_init_user_data_file.as_deref(),
        )
        .map_err(|err| match err {
            CloudInitError::BothProvided => ConfigError::CloudInit(String::from(concat!(
                "cloud-init user-data can be provided either inline or via a file, not both; ",
                "set only one of SCW_CLOUD_INIT_USER_DATA or SCW_CLOUD_INIT_USER_DATA_FILE ",
                "(or cloud_init_user_data / cloud_init_user_data_file in [scaleway])"
            ))),
            CloudInitError::InlineEmpty => ConfigError::CloudInit(String::from(concat!(
                "cloud-init user-data must not be empty; set SCW_CLOUD_INIT_USER_DATA ",
                "(or cloud_init_user_data in [scaleway])"
            ))),
            CloudInitError::FilePathEmpty | CloudInitError::FileEmpty => {
                ConfigError::CloudInit(String::from(concat!(
                    "cloud-init user-data file must not be empty; set SCW_CLOUD_INIT_USER_DATA_FILE ",
                    "(or cloud_init_user_data_file in [scaleway])"
                )))
            }
            CloudInitError::FileRead { path, message } => ConfigError::CloudInitFileRead {
                path,
                message,
            },
        })
    }

    /// Performs semantic validation on required fields. Error messages include
    /// guidance on how to provide missing values via environment variables or
    /// configuration files.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::MissingField`] when a required field is empty.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_required_fields()?;
        let _ = self.resolve_cloud_init_user_data()?;
        Ok(())
    }

    fn validate_required_fields(&self) -> Result<(), ConfigError> {
        Self::require_field(
            &self.secret_key,
            &FieldMetadata::new(
                "Scaleway API secret key",
                "SCW_SECRET_KEY",
                "secret_key",
                SCALEWAY_SECTION,
            ),
        )?;
        Self::require_field(
            &self.default_project_id,
            &FieldMetadata::new(
                "Scaleway project ID",
                "SCW_DEFAULT_PROJECT_ID",
                "default_project_id",
                SCALEWAY_SECTION,
            ),
        )?;
        Self::require_field(
            &self.default_image,
            &FieldMetadata::new(
                "VM image",
                "SCW_DEFAULT_IMAGE",
                "default_image",
                SCALEWAY_SECTION,
            ),
        )?;
        Self::require_field(
            &self.default_instance_type,
            &FieldMetadata::new(
                "instance type",
                "SCW_DEFAULT_INSTANCE_TYPE",
                "default_instance_type",
                SCALEWAY_SECTION,
            ),
        )?;
        Self::require_field(
            &self.default_zone,
            &FieldMetadata::new(
                "availability zone",
                "SCW_DEFAULT_ZONE",
                "default_zone",
                SCALEWAY_SECTION,
            ),
        )?;
        Self::require_field(
            &self.default_architecture,
            &FieldMetadata::new(
                "CPU architecture",
                "SCW_DEFAULT_ARCHITECTURE",
                "default_architecture",
                SCALEWAY_SECTION,
            ),
        )?;
        Ok(())
    }
}

/// Errors raised during configuration loading and validation.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ConfigError {
    /// Indicates a required configuration field is empty or missing.
    #[error("missing configuration field: {0}")]
    MissingField(String),
    /// Surfaces errors from the `ortho-config` loader.
    #[error("configuration parsing failed: {0}")]
    Parse(String),
    /// Raised when cloud-init user-data configuration is invalid.
    #[error("{0}")]
    CloudInit(String),
    /// Raised when reading cloud-init user-data from a file fails.
    #[error("failed to read cloud-init user-data file {path}: {message}")]
    CloudInitFileRead {
        /// Path to the user-data file.
        path: String,
        /// Underlying read error message.
        message: String,
    },
}

impl From<ortho_config::OrthoError> for ConfigError {
    fn from(value: ortho_config::OrthoError) -> Self {
        Self::Parse(value.to_string())
    }
}
