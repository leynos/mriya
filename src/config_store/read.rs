//! Reading the configuration document and its volume setting.

use std::io;

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use ortho_config::toml;

use super::{ConfigStoreError, SCALEWAY_SECTION, VOLUME_KEY};

/// Splits `path` into its directory (`.` when there is none) and file name.
///
/// Capability-based filesystem access opens the directory first and then
/// refers to the file by name.
///
/// # Errors
///
/// Returns `InvalidStructure` when `path` has no file name.
pub(super) fn split_config_path(path: &Utf8Path) -> Result<(&Utf8Path, &str), ConfigStoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .unwrap_or_else(|| Utf8Path::new("."));
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
pub(super) fn path_exists(path: &Utf8Path) -> Result<bool, ConfigStoreError> {
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

/// Reads the configuration file, or `None` when it does not exist.
pub(super) fn read_config_if_present(path: &Utf8Path) -> Result<Option<String>, ConfigStoreError> {
    let (parent, file_name) = split_config_path(path)?;
    let dir =
        Dir::open_ambient_dir(parent, ambient_authority()).map_err(|err| ConfigStoreError::Io {
            path: parent.to_path_buf(),
            message: err.to_string(),
        })?;
    match dir.read_to_string(file_name) {
        Ok(contents) => Ok(Some(contents)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        }),
    }
}

/// Reads the whole configuration file as text.
pub(super) fn read_config(path: &Utf8Path) -> Result<String, ConfigStoreError> {
    read_config_if_present(path)?.ok_or_else(|| ConfigStoreError::Io {
        path: path.to_path_buf(),
        message: String::from("configuration file does not exist"),
    })
}

/// Parses `contents` as TOML, treating blank content as an empty table so a
/// new file can be created from scratch.
pub(super) fn parse_toml(path: &Utf8Path, contents: &str) -> Result<toml::Value, ConfigStoreError> {
    if contents.trim().is_empty() {
        return Ok(toml::Value::Table(toml::value::Table::new()));
    }

    toml::from_str(contents).map_err(|err| ConfigStoreError::Parse {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}

/// Extracts `scaleway.default_volume_id`, trimmed, from the parsed document.
///
/// Returns `None` when the section or key is absent.
///
/// # Errors
///
/// Returns `InvalidStructure` when the root or section is not a table or
/// the key is not a string.
pub(super) fn read_volume_id(
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
