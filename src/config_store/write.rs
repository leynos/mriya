//! Atomic replacement of the configuration document.

use std::io::Write;

use camino::Utf8Path;
use cap_std::{ambient_authority, fs::OpenOptions, fs_utf8::Dir};
use ortho_config::toml;

use super::{
    ConfigStoreError,
    access::{Names, ensure_owner, inherit_permissions, refuse_symlink},
    split_config_path,
};

/// Renders `value` and atomically replaces the file, creating parent
/// directories.
///
/// The whole document is re-rendered, so comments and formatting in an
/// existing file are not preserved. The document is written to a temporary
/// file in the same directory and renamed over the target: a failed write then
/// leaves the previous file intact, so a rollback cannot delete a volume that
/// the configuration already names.
pub(super) fn write_config(path: &Utf8Path, value: &toml::Value) -> Result<(), ConfigStoreError> {
    // macOS passes access ACLs to new files independently of the mode, and
    // their native representation is not copied here; refuse rather than risk
    // exposing or dropping access to a file that holds a secret key.
    if cfg!(target_os = "macos") {
        return Err(ConfigStoreError::AccessControlUnsupported {
            path: path.to_path_buf(),
        });
    }
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

    // A short fixed prefix keeps the name within a 255-byte component even
    // when the configuration basename is long; the suffix keeps it unique so
    // `create_new` never reuses a stale or planted temporary file.
    let temp_name = format!(
        ".mriya-toml-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    );
    let names = Names {
        target: file_name,
        replacement: &temp_name,
    };
    refuse_symlink(&dir, &names, path)?;
    write_private(&dir, &names, rendered.as_bytes(), path)?;
    let mut temporary = TempFile {
        dir: &dir,
        name: names.replacement,
        armed: true,
    };
    ensure_owner(&dir, &names, path)?;
    inherit_permissions(&dir, &names, path)?;
    dir.rename(names.replacement, &dir, names.target)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    temporary.disarm();
    // The directory entry itself must be durable, or a crash can restore the
    // old configuration after the new volume already exists.
    sync_directory(parent, path)
}

/// Flushes a directory entry to disk.
///
/// The capability `Dir` is opened with `O_PATH` on Linux and cannot be
/// fsynced, so the directory is reopened read-only for this one call.
fn sync_directory(parent: &Utf8Path, path: &Utf8Path) -> Result<(), ConfigStoreError> {
    use rustix::fs::{CWD, Mode, OFlags, fsync, openat};

    let directory = openat(
        CWD,
        parent.as_std_path(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_error| ConfigStoreError::NotDurable {
        path: path.to_path_buf(),
    })?;
    fsync(&directory).map_err(|_error| ConfigStoreError::NotDurable {
        path: path.to_path_buf(),
    })?;
    Ok(())
}

/// Removes the temporary file unless the rename has published it.
struct TempFile<'a> {
    /// Directory holding the temporary file.
    dir: &'a Dir,
    /// Temporary file name.
    name: &'a str,
    /// Whether the file still needs removing.
    armed: bool,
}

impl TempFile<'_> {
    /// Stops the guard from removing the file.
    const fn disarm(&mut self) { self.armed = false; }
}

impl Drop for TempFile<'_> {
    fn drop(&mut self) {
        if self.armed {
            drop(self.dir.remove_file(self.name));
        }
    }
}

/// Writes `contents` to a new file that only the owner can read.
///
/// The document can hold a secret key, so it must not be readable through the
/// default umask-derived mode even briefly; the target's mode is applied just
/// before the rename.
fn write_private(
    dir: &Dir,
    names: &Names<'_>,
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
    let mut file =
        dir.open_with(names.replacement, &options)
            .map_err(|err| ConfigStoreError::Io {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;
    if let Err(error) = file.write_all(contents) {
        drop(file);
        // A partial document can hold credentials; never leave it behind.
        drop(dir.remove_file(names.replacement));
        return Err(ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }
    // The requested mode is masked by the process umask and by any default
    // ACL, so set it explicitly; the next run must be able to read the ID.
    #[cfg(unix)]
    if let Err(error) = rustix::fs::fchmod(&file, rustix::fs::Mode::from_bits_truncate(0o600)) {
        drop(file);
        drop(dir.remove_file(names.replacement));
        return Err(ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }
    // Flush the contents and metadata before the rename publishes the name.
    if let Err(error) = rustix::fs::fsync(&file) {
        drop(file);
        drop(dir.remove_file(names.replacement));
        return Err(ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }
    Ok(())
}
