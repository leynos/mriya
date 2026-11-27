//! Init configuration layering and validation.

use std::ffi::OsString;

use ortho_config::OrthoConfig;
use serde::Deserialize;

use super::error::InitConfigError;
use crate::volume::{MIN_VOLUME_SIZE_GB, VolumeTier};

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
    /// Size of the cache volume in decimal gigabytes (10^9 bytes).
    #[ortho_config(default = 20)]
    pub volume_size_gb: u32,
    /// Block Storage performance tier: `sbs_5k` (default) or `sbs_15k`.
    #[ortho_config(default = "sbs_5k".to_owned())]
    pub volume_tier: String,
}

impl InitConfig {
    /// Loads init configuration without parsing CLI arguments.
    ///
    /// # Returns
    ///
    /// The merged configuration; call [`InitConfig::validate`] before use.
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
    /// # Returns
    ///
    /// `Ok(())` when the size meets the provider minimum and the tier is
    /// supported.
    ///
    /// # Errors
    ///
    /// Returns [`InitConfigError::InvalidVolumeSize`] when the configured
    /// volume size is below the provider's 5 GB minimum, and
    /// [`InitConfigError::InvalidVolumeTier`] when the tier is not `sbs_5k` or
    /// `sbs_15k`.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::InitConfig;
    ///
    /// let config = InitConfig {
    ///     volume_size_gb: 4,
    ///     volume_tier: String::from("sbs_5k"),
    /// };
    /// assert!(config.validate().is_err());
    ///
    /// let config = InitConfig {
    ///     volume_size_gb: 5,
    ///     volume_tier: String::from("sbs_15k"),
    /// };
    /// assert!(config.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), InitConfigError> {
        if u64::from(self.volume_size_gb) < MIN_VOLUME_SIZE_GB {
            return Err(InitConfigError::InvalidVolumeSize);
        }
        VolumeTier::parse(&self.volume_tier)?;
        Ok(())
    }

    /// Returns the parsed performance tier.
    ///
    /// # Returns
    ///
    /// The configured tier, ready for a volume request.
    ///
    /// # Errors
    ///
    /// Returns [`InitConfigError::InvalidVolumeTier`] for an unsupported value.
    pub fn tier(&self) -> Result<VolumeTier, InitConfigError> {
        Ok(VolumeTier::parse(&self.volume_tier)?)
    }
}
