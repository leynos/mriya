//! Persistent configuration file updates for Mriya.

use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::{ambient_authority, fs_utf8::Dir};
use ortho_config::ConfigDiscovery;
use thiserror::Error;

use ortho_config::toml;

const APP_NAME: &str = "mriya";
const CONFIG_ENV_VAR: &str = "MRIYA_CONFIG_PATH";
const CONFIG_FILE_NAME: &str = "mriya.toml";
const DOTFILE_NAME: &str = ".mriya.toml";
const PROJECT_FILE_NAME: &str = "mriya.toml";
const SCALEWAY_SECTION: &str = "scaleway";
const VOLUME_KEY: &str = "default_volume_id";

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
    pub const fn with_discovery(discovery: ConfigDiscovery) -> Self {
        Self { discovery }
    }

    fn resolve_target(&self) -> Result<ConfigTarget, ConfigStoreError> {
        let candidates = self.discovery.utf8_candidates();
        if candidates.is_empty() {
            return Err(ConfigStoreError::NoCandidates);
        }

        for candidate in &candidates {
            if path_exists(candidate)? {
                return Ok(ConfigTarget {
                    path: candidate.clone(),
                    exists: true,
                });
            }
        }

        let fallback = candidates
            .last()
            .cloned()
            .ok_or(ConfigStoreError::NoCandidates)?;
        Ok(ConfigTarget {
            path: fallback,
            exists: false,
        })
    }
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self::new()
    }
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
        let contents = if target.exists {
            read_config(&target.path)?
        } else {
            String::new()
        };

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

#[derive(Clone, Debug)]
struct ConfigTarget {
    path: Utf8PathBuf,
    exists: bool,
}

fn path_exists(path: &Utf8Path) -> Result<bool, ConfigStoreError> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: String::from("configuration file path is missing a filename"),
        })?;

    match Dir::open_ambient_dir(parent, ambient_authority()) {
        Ok(dir) => dir
            .try_exists(file_name)
            .map_err(|err| ConfigStoreError::Io {
                path: path.to_path_buf(),
                message: err.to_string(),
            }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(ConfigStoreError::Io {
            path: parent.to_path_buf(),
            message: err.to_string(),
        }),
    }
}

fn read_config(path: &Utf8Path) -> Result<String, ConfigStoreError> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: String::from("configuration file path is missing a filename"),
        })?;

    let dir =
        Dir::open_ambient_dir(parent, ambient_authority()).map_err(|err| ConfigStoreError::Io {
            path: parent.to_path_buf(),
            message: err.to_string(),
        })?;

    dir.read_to_string(file_name)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })
}

fn parse_toml(path: &Utf8Path, contents: &str) -> Result<toml::Value, ConfigStoreError> {
    if contents.trim().is_empty() {
        return Ok(toml::Value::Table(toml::value::Table::new()));
    }

    toml::from_str(contents).map_err(|err| ConfigStoreError::Parse {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}

fn read_volume_id(
    path: &Utf8Path,
    value: &toml::Value,
) -> Result<Option<String>, ConfigStoreError> {
    let table = value
        .as_table()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: String::from("configuration root is not a table"),
        })?;

    let Some(section) = table.get(SCALEWAY_SECTION) else {
        return Ok(None);
    };

    let section_table = section
        .as_table()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: format!("[{SCALEWAY_SECTION}] must be a table"),
        })?;

    section_table.get(VOLUME_KEY).map_or(Ok(None), |raw| {
        raw.as_str()
            .map(|id| Some(id.trim().to_owned()))
            .ok_or_else(|| ConfigStoreError::InvalidStructure {
                path: path.to_path_buf(),
                message: format!("{SCALEWAY_SECTION}.{VOLUME_KEY} must be a string"),
            })
    })
}

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

fn write_config(path: &Utf8Path, value: &toml::Value) -> Result<(), ConfigStoreError> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    Dir::create_ambient_dir_all(parent, ambient_authority()).map_err(|err| {
        ConfigStoreError::Io {
            path: parent.to_path_buf(),
            message: err.to_string(),
        }
    })?;

    let file_name = path
        .file_name()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: String::from("configuration file path is missing a filename"),
        })?;
    let dir =
        Dir::open_ambient_dir(parent, ambient_authority()).map_err(|err| ConfigStoreError::Io {
            path: parent.to_path_buf(),
            message: err.to_string(),
        })?;

    let rendered = toml::to_string_pretty(value).map_err(|err| ConfigStoreError::Parse {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;

    dir.write(file_name, rendered)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn discovery_for_path(path: &Utf8Path) -> ConfigDiscovery {
        let root = path
            .parent()
            .expect("temp path should have a parent directory");
        ConfigDiscovery::builder(APP_NAME)
            .env_var(CONFIG_ENV_VAR)
            .config_file_name(CONFIG_FILE_NAME)
            .dotfile_name(DOTFILE_NAME)
            .project_file_name(PROJECT_FILE_NAME)
            .clear_project_roots()
            .add_project_root(root)
            .build()
    }

    fn temp_config_path(tmp: &TempDir) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(tmp.path().join("mriya.toml"))
            .unwrap_or_else(|err| panic!("temp path should be utf8: {}", err.display()))
    }

    #[test]
    fn write_volume_id_creates_config_file() {
        let tmp = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = temp_config_path(&tmp);
        let store = ConfigStore::with_discovery(discovery_for_path(&path));

        let written_path = store
            .write_volume_id("vol-123", true)
            .unwrap_or_else(|err| panic!("write volume id: {err}"));

        assert_eq!(written_path, path);
        let contents = read_config(&path).unwrap_or_else(|err| panic!("read config: {err}"));
        let value =
            parse_toml(&path, &contents).unwrap_or_else(|err| panic!("parse config: {err}"));
        let volume_id =
            read_volume_id(&path, &value).unwrap_or_else(|err| panic!("extract volume id: {err}"));
        assert_eq!(volume_id, Some(String::from("vol-123")));
    }

    #[test]
    fn write_volume_id_rejects_existing_without_force() {
        let tmp = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = temp_config_path(&tmp);
        let store = ConfigStore::with_discovery(discovery_for_path(&path));
        store
            .write_volume_id("vol-123", true)
            .unwrap_or_else(|err| panic!("seed config: {err}"));

        let Err(err) = store.write_volume_id("vol-456", false) else {
            panic!("overwrite should fail without force");
        };

        let ConfigStoreError::VolumeAlreadyConfigured { volume_id } = err else {
            panic!("expected VolumeAlreadyConfigured error");
        };
        assert_eq!(volume_id, "vol-123");
    }

    #[test]
    fn write_volume_id_overwrites_when_forced() {
        let tmp = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = temp_config_path(&tmp);
        let store = ConfigStore::with_discovery(discovery_for_path(&path));
        store
            .write_volume_id("vol-123", true)
            .unwrap_or_else(|err| panic!("seed config: {err}"));

        store
            .write_volume_id("vol-456", true)
            .unwrap_or_else(|err| panic!("overwrite config: {err}"));

        let contents = read_config(&path).unwrap_or_else(|err| panic!("read config: {err}"));
        let value =
            parse_toml(&path, &contents).unwrap_or_else(|err| panic!("parse config: {err}"));
        let volume_id =
            read_volume_id(&path, &value).unwrap_or_else(|err| panic!("extract volume id: {err}"));
        assert_eq!(volume_id, Some(String::from("vol-456")));
    }
}
