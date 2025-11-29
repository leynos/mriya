//! Configuration loading via `ortho-config`.

use crate::backend::InstanceRequest;
use ortho_config::OrthoConfig;
use serde::Deserialize;
use thiserror::Error;

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
    #[ortho_config(default = "fr-par-1")]
    pub default_zone: String,
    /// Commercial type for new instances. Defaults to `DEV1-S` to minimise
    /// cost during integration tests.
    #[ortho_config(default = "DEV1-S")]
    pub default_instance_type: String,
    /// Human friendly image label (for example `Ubuntu 24.04 Noble Numbat`).
    #[ortho_config(default = "Ubuntu 24.04 Noble Numbat")]
    pub default_image: String,
    /// CPU architecture used to select the correct image variant.
    #[ortho_config(default = "x86_64")]
    pub default_architecture: String,
}

impl ScalewayConfig {
    /// Loads configuration using the `ortho-config` derive. Values merge
    /// defaults, configuration files, environment variables, and CLI flags in
    /// that order of precedence.
    #[must_use]
    pub fn load_from_sources() -> Result<Self, ConfigError> {
        Ok(Self::load()?)
    }

    /// Builds an [`InstanceRequest`] using the configured defaults.
    #[must_use]
    pub fn as_request(&self) -> Result<InstanceRequest, ConfigError> {
        self.validate()?;
        Ok(InstanceRequest::new(
            &self.default_image,
            &self.default_instance_type,
            &self.default_zone,
            &self.default_project_id,
            self.default_organization_id.clone(),
            &self.default_architecture,
        ))
    }

    /// Performs semantic validation on required fields.
    #[must_use]
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.secret_key.trim().is_empty() {
            return Err(ConfigError::MissingField("SCW_SECRET_KEY".to_owned()));
        }
        if self.default_project_id.trim().is_empty() {
            return Err(ConfigError::MissingField(
                "SCW_DEFAULT_PROJECT_ID".to_owned(),
            ));
        }
        if self.default_image.trim().is_empty() {
            return Err(ConfigError::MissingField("default_image".to_owned()));
        }
        if self.default_instance_type.trim().is_empty() {
            return Err(ConfigError::MissingField(
                "default_instance_type".to_owned(),
            ));
        }
        if self.default_zone.trim().is_empty() {
            return Err(ConfigError::MissingField("default_zone".to_owned()));
        }
        if self.default_architecture.trim().is_empty() {
            return Err(ConfigError::MissingField(
                "default_architecture".to_owned(),
            ));
        }
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
}

impl From<ortho_config::OrthoError> for ConfigError {
    fn from(value: ortho_config::OrthoError) -> Self {
        Self::Parse(value.to_string())
    }
}
