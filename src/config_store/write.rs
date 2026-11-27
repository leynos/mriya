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

    refuse_symlink(&dir, file_name, path)?;

    let rendered = toml::to_string_pretty(value).map_err(|err| ConfigStoreError::Parse {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;

    // A short fixed prefix keeps the name within a 255-byte component even
    // when the configuration basename is long; the suffix keeps it unique so
    // `create_new` never reuses a stale or planted temporary file.
    let temp_name = format!(
        ".mriya-toml-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    );
    write_private(&dir, &temp_name, rendered.as_bytes(), path)?;
    ensure_owner(&dir, file_name, &temp_name, path)?;
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
    if let Err(error) = file.write_all(contents) {
        drop(file);
        // A partial document can hold credentials; never leave it behind.
        drop(dir.remove_file(name));
        return Err(ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }
    Ok(())
}

/// Refuses to replace a symbolic-link configuration file.
///
/// A rename replaces the directory entry, so writing through the link would
/// silently turn it into a regular file and stop updating its referent.
fn refuse_symlink(dir: &Dir, file_name: &str, path: &Utf8Path) -> Result<(), ConfigStoreError> {
    match dir.symlink_metadata(file_name) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ConfigStoreError::Symlinked {
            path: path.to_path_buf(),
        }),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        }),
    }
}

/// Refuses a replacement that would change the configuration file's owner.
///
/// A privileged update of a user-owned file would otherwise leave the new
/// file owned by the writer, locking the original owner out.
#[cfg(unix)]
fn ensure_owner(dir: &Dir, from: &str, to: &str, path: &Utf8Path) -> Result<(), ConfigStoreError> {
    use cap_std::fs::MetadataExt;

    let (Ok(target), Ok(replacement)) = (dir.metadata(from), dir.metadata(to)) else {
        return Ok(());
    };
    if target.uid() == replacement.uid() {
        return Ok(());
    }
    // The replacement is discarded by the caller; never publish it.
    drop(dir.remove_file(to));
    Err(ConfigStoreError::OwnershipChanged {
        path: path.to_path_buf(),
    })
}

/// Leaves ownership unchanged on non-Unix platforms.
#[cfg(not(unix))]
fn ensure_owner(
    _dir: &Dir,
    _from: &str,
    _to: &str,
    _path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
    Ok(())
}

/// Copies the target's permissions onto its replacement before the rename.
///
/// A new file keeps the restricted mode `write_private` applied, because no
/// target mode exists to inherit.
#[cfg(unix)]
fn inherit_permissions(dir: &Dir, from: &str, to: &str) {
    use cap_std::fs::{MetadataExt, Permissions, PermissionsExt};

    let (Ok(target), Ok(replacement)) = (dir.metadata(from), dir.metadata(to)) else {
        return;
    };
    // A new file takes its group from the process or the parent directory, not
    // from the file it replaces. Granting the target's group access while the
    // group differs would expose the secret key to the wrong group.
    if target.gid() != replacement.gid() {
        return;
    }
    let mode = target.permissions().mode();
    drop(dir.set_permissions(to, Permissions::from_mode(mode)));
}

/// Leaves the replacement's default permissions on non-Unix platforms.
#[cfg(not(unix))]
fn inherit_permissions(_dir: &Dir, _from: &str, _to: &str) {}
