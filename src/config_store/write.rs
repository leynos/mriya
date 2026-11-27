//! Atomic replacement of the configuration document.

use std::io::Write;

use camino::Utf8Path;
use cap_std::{ambient_authority, fs::OpenOptions, fs_utf8::Dir};
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

    // A unique name avoids reusing a stale or planted temporary file, whose
    // mode `create_new` would otherwise inherit.
    let temp_name = format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    );
    write_private(&dir, &temp_name, rendered.as_bytes(), path)?;
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

/// Writes `contents` to a new file that only the owner can read.
///
/// The document can hold a secret key, so it must not be readable through the
/// default umask-derived mode even briefly; the target's mode is applied just
/// before the rename.
fn write_private(
    dir: &Dir,
    name: &str,
    contents: &[u8],
    path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = dir
        .open_with(name, &options)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    file.write_all(contents)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })
}

/// Copies the target's permissions onto its replacement before the rename.
///
/// A new file keeps the restricted mode `write_private` applied, because no
/// target mode exists to inherit.
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
