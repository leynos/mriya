//! Atomic replacement of the configuration document.

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use ortho_config::toml;

use super::{ConfigStoreError, split_config_path};

/// Renders `value` and atomically replaces the file, creating parent
/// directories.
///
/// The whole document is re-rendered, so comments and formatting in an
/// existing file are not preserved. The document is written to a temporary
/// file in the same directory and renamed over the target: a failed write then
/// leaves the previous file intact, so a rollback cannot delete a volume that
/// the configuration already names.
pub(super) fn write_config(path: &Utf8Path, value: &toml::Value) -> Result<(), ConfigStoreError> {
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

    let temp_name = format!(".{file_name}.tmp-{}", std::process::id());
    dir.write(&temp_name, rendered)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    inherit_permissions(&dir, file_name, &temp_name);
    dir.rename(&temp_name, &dir, file_name).map_err(|err| {
        // The temporary file is left behind only on this failure path.
        drop(dir.remove_file(&temp_name));
        ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        }
    })
}

/// Copies the target's permissions onto its replacement before the rename.
///
/// Rewriting a configuration file that holds a secret key must not widen its
/// mode, which a freshly created temporary file would otherwise do.
#[cfg(unix)]
fn inherit_permissions(dir: &Dir, from: &str, to: &str) {
    use cap_std::fs::{Permissions, PermissionsExt};

    let Ok(metadata) = dir.metadata(from) else {
        return;
    };
    let mode = metadata.permissions().mode();
    drop(dir.set_permissions(to, Permissions::from_mode(mode)));
}

/// Leaves the replacement's default permissions on non-Unix platforms.
#[cfg(not(unix))]
fn inherit_permissions(_dir: &Dir, _from: &str, _to: &str) {}
