//! Atomic replacement of the configuration document.

use std::io::Write;

use camino::Utf8Path;
use cap_std::{ambient_authority, fs::OpenOptions, fs_utf8::Dir};
use ortho_config::toml;

use super::{ConfigStoreError, split_config_path};

/// The target and temporary file names involved in one replacement.
struct Names<'a> {
    /// Existing configuration file name.
    target: &'a str,
    /// Temporary replacement file name.
    replacement: &'a str,
}

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
    inherit_permissions(&dir, &names);
    dir.rename(names.replacement, &dir, names.target)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    temporary.disarm();
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
    Ok(())
}

/// Refuses to replace a symbolic-link configuration file.
///
/// A rename replaces the directory entry, so writing through the link would
/// silently turn it into a regular file and stop updating its referent.
fn refuse_symlink(dir: &Dir, names: &Names<'_>, path: &Utf8Path) -> Result<(), ConfigStoreError> {
    match dir.symlink_metadata(names.target) {
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
fn ensure_owner(dir: &Dir, names: &Names<'_>, path: &Utf8Path) -> Result<(), ConfigStoreError> {
    use cap_std::fs::MetadataExt;

    let target = match dir.metadata(names.target) {
        Ok(target) => target,
        // Only an absent target is the new-file case.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            drop(dir.remove_file(names.replacement));
            return Err(ConfigStoreError::Io {
                path: path.to_path_buf(),
                message: err.to_string(),
            });
        }
    };
    let replacement = dir
        .metadata(names.replacement)
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    if target.uid() != replacement.uid() {
        // The replacement is discarded by the caller; never publish it.
        drop(dir.remove_file(names.replacement));
        return Err(ConfigStoreError::OwnershipChanged {
            path: path.to_path_buf(),
        });
    }
    // ACL group entries refer to the owning group, and applying the target
    // mode without its group would drop existing readers, so a differing group
    // refuses the replacement.
    if target.gid() != replacement.gid() {
        drop(dir.remove_file(names.replacement));
        return Err(ConfigStoreError::GroupChanged {
            path: path.to_path_buf(),
        });
    }
    preserve_access_controls(dir, names, &target, path)
}

/// Preserves the target's access controls on the replacement.
#[cfg(target_os = "linux")]
fn preserve_access_controls(
    dir: &Dir,
    names: &Names<'_>,
    _target: &cap_std::fs::Metadata,
    path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
    copy_acl(dir, names, path)
}

/// Refuses a replacement whose native ACLs cannot be preserved.
///
/// macOS does not use the POSIX `system.posix_acl_access` attribute, so this
/// platform only allows replacing a file that grants no group or other
/// access; such a file's mode fully describes its readers.
#[cfg(target_os = "macos")]
fn preserve_access_controls(
    dir: &Dir,
    names: &Names<'_>,
    target: &cap_std::fs::Metadata,
    path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
    use cap_std::fs::PermissionsExt;

    if target.permissions().mode() & 0o077 != 0 {
        drop(dir.remove_file(names.replacement));
        return Err(ConfigStoreError::AccessControlUnsupported {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Leaves access controls unchanged on other platforms.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn preserve_access_controls(
    _dir: &Dir,
    _names: &Names<'_>,
    _target: &cap_std::fs::Metadata,
    _path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
    Ok(())
}

/// Copies the target's access ACL onto the replacement.
///
/// A configuration holding a secret key may use an ACL to grant one named
/// user access while denying the owning group. Publishing a replacement
/// without that ACL would widen access, so an unreadable or uncopyable ACL
/// fails the update instead.
#[cfg(unix)]
fn copy_acl(dir: &Dir, names: &Names<'_>, path: &Utf8Path) -> Result<(), ConfigStoreError> {
    use rustix::fs::{Mode, OFlags, XattrFlags, fgetxattr, fremovexattr, fsetxattr};

    let open = |name: &str| {
        rustix::fs::openat(dir, name, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty()).map_err(
            |err| ConfigStoreError::Io {
                path: path.to_path_buf(),
                message: err.to_string(),
            },
        )
    };
    let target = open(names.target)?;
    let replacement = open(names.replacement)?;
    let mut buffer = vec![0_u8; 4096];
    let size = match fgetxattr(&target, "system.posix_acl_access", &mut buffer) {
        Ok(size) => size,
        // No ACL is set. A default ACL on the directory may still have been
        // inherited by the replacement, so remove it rather than leave it to
        // be enabled by the target mode.
        Err(err) if is_missing_attribute(err) => {
            let _cleared = fremovexattr(&replacement, "system.posix_acl_access");
            return Ok(());
        }
        Err(err) => {
            drop(dir.remove_file(names.replacement));
            return Err(ConfigStoreError::Io {
                path: path.to_path_buf(),
                message: err.to_string(),
            });
        }
    };
    let acl = buffer.get(..size).ok_or_else(|| ConfigStoreError::Io {
        path: path.to_path_buf(),
        message: String::from("the ACL size exceeds the read buffer"),
    })?;
    fsetxattr(
        &replacement,
        "system.posix_acl_access",
        acl,
        XattrFlags::empty(),
    )
    .map_err(|err| {
        drop(dir.remove_file(names.replacement));
        ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        }
    })
}

/// Reports whether `err` means the file has no access ACL.
#[cfg(target_os = "macos")]
const fn is_missing_attribute(err: rustix::io::Errno) -> bool { err == rustix::io::Errno::NOATTR }

/// Reports whether `err` means the file or filesystem has no access ACL.
#[cfg(not(target_os = "macos"))]
const fn is_missing_attribute(err: rustix::io::Errno) -> bool {
    matches!(err, rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP)
}

/// Leaves ownership unchanged on non-Unix platforms.
#[cfg(not(unix))]
fn ensure_owner(_dir: &Dir, _names: &Names<'_>, _path: &Utf8Path) -> Result<(), ConfigStoreError> {
    Ok(())
}

/// Copies the target's permissions onto its replacement before the rename.
///
/// A new file keeps the restricted mode `write_private` applied, because no
/// target mode exists to inherit.
#[cfg(unix)]
fn inherit_permissions(dir: &Dir, names: &Names<'_>) {
    use cap_std::fs::{MetadataExt, Permissions, PermissionsExt};

    let (Ok(target), Ok(replacement)) =
        (dir.metadata(names.target), dir.metadata(names.replacement))
    else {
        return;
    };
    // The ACL is copied first; a differing group would then widen the mode's
    // group access, so the restricted mode stays in place.
    if target.gid() != replacement.gid() {
        return;
    }
    let mode = target.permissions().mode();
    drop(dir.set_permissions(names.replacement, Permissions::from_mode(mode)));
}

/// Leaves the replacement's default permissions on non-Unix platforms.
#[cfg(not(unix))]
fn inherit_permissions(_dir: &Dir, _names: &Names<'_>) {}
