//! Persistent configuration file updates for Mriya.

use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::{ambient_authority, fs_utf8::Dir};
use ortho_config::{ConfigDiscovery, toml};
use thiserror::Error;

mod volume;

#[cfg(test)]
pub(crate) use volume::{LEGACY_SCALEWAY_SECTION, VOLUME_KEY};
use volume::{read_volume_id, write_volume_id_value};

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
    /// Raised when the discovered configuration file is not TOML.
    ///
    /// The loader can read YAML and JSON5, but this store only edits TOML, so
    /// it refuses rather than parsing another format as TOML and reporting a
    /// misleading syntax error.
    #[error("cannot update {path}: only TOML configuration files are supported (found {format})")]
    UnsupportedFormat {
        /// Configuration file that would have been edited.
        path: Utf8PathBuf,
        /// File extension the store found.
        format: String,
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

    /// Picks the first discovered candidate that exists, or the last TOML
    /// candidate (marked as not existing) when none do.
    ///
    /// The loader can read YAML and JSON5 as well as TOML, so an existing
    /// non-TOML file may be the one a run reads. This store only edits TOML,
    /// so it reports [`ConfigStoreError::UnsupportedFormat`] rather than
    /// parsing that file as TOML or writing a second file the loader would
    /// not select.
    ///
    /// # Errors
    ///
    /// Returns `NoCandidates` when discovery yields no TOML candidate, `Io`
    /// when an existence check fails, or `UnsupportedFormat` when a discovered
    /// file is not TOML.
    fn resolve_target(&self) -> Result<ConfigTarget, ConfigStoreError> {
        let candidates = self.discovery.utf8_candidates();
        for candidate in &candidates {
            if path_exists(candidate)? {
                return existing_target(candidate);
            }
        }

        let Some(fallback) = candidates
            .iter()
            .rev()
            .find(|path| parses_as_toml(path))
            .cloned()
        else {
            return Err(ConfigStoreError::NoCandidates);
        };
        Ok(ConfigTarget {
            path: fallback,
            exists: false,
        })
    }
}

/// The target for a discovered file that exists, refusing a non-TOML format.
fn existing_target(path: &Utf8Path) -> Result<ConfigTarget, ConfigStoreError> {
    if parses_as_toml(path) {
        return Ok(ConfigTarget {
            path: path.to_path_buf(),
            exists: true,
        });
    }
    Err(ConfigStoreError::UnsupportedFormat {
        path: path.to_path_buf(),
        format: path.extension().unwrap_or("unknown").to_owned(),
    })
}

/// Whether the loader parses `path` as TOML.
///
/// `ortho_config` selects YAML for `yaml`/`yml` and JSON5 for `json`/`json5`;
/// every other extension, including none, is parsed as TOML. The store mirrors
/// that classification so it refuses exactly the files it cannot edit.
fn parses_as_toml(path: &Utf8Path) -> bool {
    const NON_TOML: [&str; 4] = ["json", "json5", "yaml", "yml"];
    let Some(extension) = path.extension() else {
        return true;
    };
    !NON_TOML
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
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

/// The configuration file chosen for a read or write.
#[derive(Clone, Debug)]
struct ConfigTarget {
    /// Location of the file, whether or not it exists yet.
    path: Utf8PathBuf,
    /// Whether the file was found on disk; a missing file is treated as empty
    /// and created on write.
    exists: bool,
}

/// Splits `path` into its directory (`.` when there is none) and file name.
///
/// Capability-based filesystem access opens the directory first and then
/// refers to the file by name.
///
/// # Errors
///
/// Returns `InvalidStructure` when `path` has no file name.
fn split_config_path(path: &Utf8Path) -> Result<(&Utf8Path, &str), ConfigStoreError> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: String::from("configuration file path is missing a filename"),
        })?;
    Ok((parent, file_name))
}

/// Reports whether `path` exists, treating a missing parent directory as
/// absence rather than an error.
fn path_exists(path: &Utf8Path) -> Result<bool, ConfigStoreError> {
    let (parent, file_name) = split_config_path(path)?;

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

/// Reads the whole configuration file as text.
fn read_config(path: &Utf8Path) -> Result<String, ConfigStoreError> {
    let (parent, file_name) = split_config_path(path)?;

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

/// Parses `contents` as TOML, treating blank content as an empty table so a
/// new file can be created from scratch.
fn parse_toml(path: &Utf8Path, contents: &str) -> Result<toml::Value, ConfigStoreError> {
    if contents.trim().is_empty() {
        return Ok(toml::Value::Table(toml::value::Table::new()));
    }

    toml::from_str(contents).map_err(|err| ConfigStoreError::Parse {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}

/// Renders `value` and writes it over the file, creating parent directories.
///
/// The whole document is re-rendered, so comments and formatting in an
/// existing file are not preserved. The write is not atomic.
fn write_config(path: &Utf8Path, value: &toml::Value) -> Result<(), ConfigStoreError> {
    let (parent, file_name) = split_config_path(path)?;
    Dir::create_ambient_dir_all(parent, ambient_authority()).map_err(|err| {
        ConfigStoreError::Io {
            path: parent.to_path_buf(),
            message: err.to_string(),
        }
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
mod tests;
