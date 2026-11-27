//! Ownership and access-control preservation for configuration writes.

use camino::Utf8Path;
use cap_std::fs_utf8::Dir;

use super::ConfigStoreError;

/// The target and temporary file names involved in one replacement.
pub(super) struct Names<'a> {
    /// Existing configuration file name.
    pub(super) target: &'a str,
    /// Temporary replacement file name.
    pub(super) replacement: &'a str,
}

/// Refuses to replace a symbolic-link configuration file.
///
/// A rename replaces the directory entry, so writing through the link would
/// silently turn it into a regular file and stop updating its referent.
pub(super) fn refuse_symlink(
    dir: &Dir,
    names: &Names<'_>,
    path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
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
pub(super) fn ensure_owner(
    dir: &Dir,
    names: &Names<'_>,
    path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
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
pub(super) fn preserve_access_controls(
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
pub(super) fn preserve_access_controls(
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
pub(super) fn preserve_access_controls(
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
#[cfg(target_os = "linux")]
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
            match fremovexattr(&replacement, "system.posix_acl_access") {
                Ok(()) | Err(rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP) => {
                    return Ok(());
                }
                Err(remove_error) => {
                    drop(dir.remove_file(names.replacement));
                    return Err(ConfigStoreError::Io {
                        path: path.to_path_buf(),
                        message: remove_error.to_string(),
                    });
                }
            }
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

/// Reports whether `err` means the file or filesystem has no access ACL.
#[cfg(target_os = "linux")]
const fn is_missing_attribute(err: rustix::io::Errno) -> bool {
    matches!(err, rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP)
}

/// Leaves ownership unchanged on non-Unix platforms.
#[cfg(not(unix))]
pub(super) fn ensure_owner(
    _dir: &Dir,
    _names: &Names<'_>,
    _path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
    Ok(())
}

/// Copies the target's permissions onto its replacement before the rename.
///
/// A new file keeps the restricted mode `write_private` applied, because no
/// target mode exists to inherit.
#[cfg(unix)]
pub(super) fn inherit_permissions(
    dir: &Dir,
    names: &Names<'_>,
    path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
    use cap_std::fs::{MetadataExt, Permissions, PermissionsExt};

    let (Ok(target), Ok(replacement)) =
        (dir.metadata(names.target), dir.metadata(names.replacement))
    else {
        return Ok(());
    };
    // A differing group was refused earlier, so the target mode can be
    // restored without widening group access.
    if target.gid() != replacement.gid() {
        return Ok(());
    }
    let mode = target.permissions().mode();
    dir.set_permissions(names.replacement, Permissions::from_mode(mode))
        .map_err(|err| ConfigStoreError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })
}

/// Leaves the replacement's default permissions on non-Unix platforms.
#[cfg(not(unix))]
pub(super) fn inherit_permissions(
    _dir: &Dir,
    _names: &Names<'_>,
    _path: &Utf8Path,
) -> Result<(), ConfigStoreError> {
    Ok(())
}
