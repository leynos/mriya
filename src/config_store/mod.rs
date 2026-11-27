//! Persistent configuration file updates for Mriya.

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::{ambient_authority, fs_utf8::Dir};
use ortho_config::{ConfigDiscovery, toml};
use thiserror::Error;

mod read;
mod write;

use read::{parse_toml, read_config, read_config_if_present, read_volume_id, split_config_path};
use write::write_config;

/// Application name that seeds `OrthoConfig` discovery paths.
const APP_NAME: &str = "mriya";
/// Environment variable that overrides the configuration file location.
const CONFIG_ENV_VAR: &str = "MRIYA_CONFIG_PATH";
/// File name searched for in the user and system configuration directories.
const CONFIG_FILE_NAME: &str = "mriya.toml";
/// Hidden file name searched for in the home directory.
const DOTFILE_NAME: &str = ".mriya.toml";
/// File name searched for in the project (working) directory.
const PROJECT_FILE_NAME: &str = "mriya.toml";
/// TOML table that holds the volume setting that this store reads and writes.
const SCALEWAY_SECTION: &str = "scaleway";
/// Key within the table that records the cache volume ID.
const VOLUME_KEY: &str = "default_volume_id";
/// Lock file that serializes the read-check-publish sequence.
const LOCK_FILE: &str = ".mriya-config.lock";

/// Errors raised while updating the configuration file.
#[derive(Debug, Error)]
pub enum ConfigStoreError {
    /// Raised when no configuration candidates are available.
    #[error("no configuration file candidates were discovered")]
    NoCandidates,
    /// Raised when file system operations fail.
    #[error("failed to access {path}: {message}")]
    Io {
        /// Path that could not be accessed.
        path: Utf8PathBuf,
        /// Human-readable error message.
        message: String,
    },
    /// Raised when parsing existing TOML content fails.
    #[error("failed to parse {path}: {message}")]
    Parse {
        /// Path that could not be parsed.
        path: Utf8PathBuf,
        /// Human-readable error message.
        message: String,
    },
    /// Raised when existing TOML has an unexpected structure.
    #[error("invalid configuration in {path}: {message}")]
    InvalidStructure {
        /// Path that had invalid content.
        path: Utf8PathBuf,
        /// Human-readable error message.
        message: String,
    },
    /// Raised when a volume ID is already configured and overwrite is disabled.
    #[error(
        "default volume ID already configured as {volume_id}; rerun with --force to replace it"
    )]
    VolumeAlreadyConfigured {
        /// Volume identifier already present in configuration.
        volume_id: String,
    },
    /// Raised when replacing the file would change its owner.
    #[error("refusing to replace {path}: the update would change the file's owner")]
    OwnershipChanged {
        /// Configuration path whose owner would change.
        path: Utf8PathBuf,
    },
    /// Raised when replacing the file would change its group and drop access.
    #[error(
        "refusing to replace {path}: the update would change the file's group and drop existing group access"
    )]
    GroupChanged {
        /// Configuration path whose group would change.
        path: Utf8PathBuf,
    },
    /// Raised when native ACLs cannot be preserved on this platform.
    #[error(
        "refusing to replace {path}: ACLs cannot be preserved here while the file grants group or other access"
    )]
    AccessControlUnsupported {
        /// Configuration path whose ACLs would be lost.
        path: Utf8PathBuf,
    },
    /// Raised when the configuration path is a symbolic link.
    #[error(
        "refusing to update {path}: it is a symbolic link; point the discovery path at the real file"
    )]
    Symlinked {
        /// Configuration path that is a symbolic link.
        path: Utf8PathBuf,
    },
    /// Raised when a volume ID is in effect from a source other than the file
    /// or the environment.
    #[error(
        "volume ID {volume_id} is already in effect; unset it or rerun with --force to create and record a new volume"
    )]
    VolumeIdInEffect {
        /// Volume identifier already in effect.
        volume_id: String,
    },
    /// Raised when the environment supplies an effective volume ID.
    #[error(
        "SCW_DEFAULT_VOLUME_ID={volume_id} is already in effect and overrides mriya.toml; unset it or rerun with --force to create and record a new volume"
    )]
    EffectiveVolumeConfigured {
        /// Volume identifier supplied by the environment.
        volume_id: String,
    },
}

/// Abstraction over configuration writers for dependency injection.
pub trait ConfigWriter {
    /// Returns the currently configured volume ID, if present.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigStoreError`] when the configuration file cannot be
    /// accessed or parsed.
    fn current_volume_id(&self) -> Result<Option<String>, ConfigStoreError>;

    /// Writes the volume ID to the configuration file.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigStoreError`] when reading or updating configuration
    /// content fails.
    fn write_volume_id(
        &self,
        volume_id: &str,
        force: bool,
    ) -> Result<Utf8PathBuf, ConfigStoreError>;
}

/// Updates `mriya.toml` using `OrthoConfig`'s discovery search order.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    /// Supplies the ordered candidate paths for the configuration file.
    discovery: ConfigDiscovery,
}

impl ConfigStore {
    /// Builds a config store using the standard Mriya discovery settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            discovery: ConfigDiscovery::builder(APP_NAME)
                .env_var(CONFIG_ENV_VAR)
                .config_file_name(CONFIG_FILE_NAME)
                .dotfile_name(DOTFILE_NAME)
                .project_file_name(PROJECT_FILE_NAME)
                .build(),
        }
    }

    /// Builds a config store using an explicit discovery configuration.
    #[must_use]
    pub const fn with_discovery(discovery: ConfigDiscovery) -> Self { Self { discovery } }

    /// Picks the first discovered candidate that exists, or the last candidate
    /// (marked as not existing) when none do.
    ///
    /// # Errors
    ///
    /// Returns `NoCandidates` when discovery yields nothing, or `Io` when an
    /// existence check fails.
    fn resolve_target(&self) -> Result<ConfigTarget, ConfigStoreError> {
        let candidates = self.discovery.utf8_candidates();
        for candidate in &candidates {
            if read::path_exists(candidate)? {
                return Ok(ConfigTarget {
                    path: candidate.clone(),
                    exists: true,
                });
            }
        }

        let Some(fallback) = candidates.last().cloned() else {
            return Err(ConfigStoreError::NoCandidates);
        };
        Ok(ConfigTarget {
            path: fallback,
            exists: false,
        })
    }
}

impl Default for ConfigStore {
    fn default() -> Self { Self::new() }
}

impl ConfigWriter for ConfigStore {
    fn current_volume_id(&self) -> Result<Option<String>, ConfigStoreError> {
        let target = self.resolve_target()?;
        if !target.exists {
            return Ok(None);
        }

        let contents = read_config(&target.path)?;
        let value = parse_toml(&target.path, &contents)?;
        read_volume_id(&target.path, &value)
    }

    fn write_volume_id(
        &self,
        volume_id: &str,
        force: bool,
    ) -> Result<Utf8PathBuf, ConfigStoreError> {
        let target = self.resolve_target()?;
        let (parent, _) = split_config_path(&target.path)?;
        Dir::create_ambient_dir_all(parent, ambient_authority()).map_err(|err| {
            ConfigStoreError::Io {
                path: parent.to_path_buf(),
                message: err.to_string(),
            }
        })?;
        let dir = Dir::open_ambient_dir(parent, ambient_authority()).map_err(|err| {
            ConfigStoreError::Io {
                path: parent.to_path_buf(),
                message: err.to_string(),
            }
        })?;
        let _lock = lock_config(&dir, &target.path)?;
        // Another writer may have published the file since discovery, so the
        // decision is made from the state read under the lock. Only a typed
        // `NotFound` is treated as an empty document.
        let contents = read_config_if_present(&target.path)?.unwrap_or_default();

        let mut value = parse_toml(&target.path, &contents)?;
        if let Some(existing) = read_volume_id(&target.path, &value)?
            && !force
        {
            return Err(ConfigStoreError::VolumeAlreadyConfigured {
                volume_id: existing,
            });
        }

        write_volume_id_value(&target.path, &mut value, volume_id)?;
        write_config(&target.path, &value)?;
        Ok(target.path)
    }
}

/// The configuration file chosen for a read or write.
#[derive(Clone, Debug)]
struct ConfigTarget {
    /// Location of the file, whether or not it exists yet.
    path: Utf8PathBuf,
    /// Whether the file was found on disk; a missing file is treated as empty
    /// and created on write.
    exists: bool,
}

/// Takes the exclusive configuration lock for this update.
///
/// The lock is held from the read that checks for an existing ID through the
/// rename that publishes the new one, so two unforced runs cannot both pass
/// the check. The lock is released when the returned file is dropped.
fn lock_config(dir: &Dir, path: &Utf8Path) -> Result<cap_std::fs_utf8::File, ConfigStoreError> {
    let mut options = cap_std::fs_utf8::OpenOptions::new();
    options.read(true).write(true).create(true);
    let file = dir
        .open_with(LOCK_FILE, &options)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).map_err(|err| {
        ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        }
    })?;
    Ok(file)
}

/// Sets `scaleway.default_volume_id` (trimmed) in `value`, creating the
/// section when needed and leaving other keys alone.
///
/// # Errors
///
/// Returns `InvalidStructure` when the root or section is not a table.
fn write_volume_id_value(
    path: &Utf8Path,
    value: &mut toml::Value,
    volume_id: &str,
) -> Result<(), ConfigStoreError> {
    let table = value
        .as_table_mut()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: String::from("configuration root is not a table"),
        })?;

    let section = table
        .entry(String::from(SCALEWAY_SECTION))
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));

    let section_table =
        section
            .as_table_mut()
            .ok_or_else(|| ConfigStoreError::InvalidStructure {
                path: path.to_path_buf(),
                message: format!("[{SCALEWAY_SECTION}] must be a table"),
            })?;

    section_table.insert(
        String::from(VOLUME_KEY),
        toml::Value::String(volume_id.trim().to_owned()),
    );
    Ok(())
}

#[cfg(test)]
mod tests;
