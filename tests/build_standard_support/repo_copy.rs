//! Copies the repository's build-tools scripts, the pins they read, and its toolchain file into a
//! sandbox, so a case can run them against a chosen `PATH` and rewrite a copy without touching the
//! tree under review. Every read and every mode goes through a `cap_std` handle on the file's own
//! directory, so the copy can only reach what that handle reaches.

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::{ambient_authority, fs::PermissionsExt as _, fs_utf8::Dir};

use super::sandbox::Sandbox;

/// What a copy takes from the repository: the scripts, the pins they read, and the toolchain file.
///
/// The toolchain file is included because the scripts locate the repository from their own
/// directory, so a copy that lacked it would look like a repository with no pinned channel and the
/// preflight would fail for the wrong reason.
const SOURCES: [&str; 3] = ["scripts", "tools/mold", "rust-toolchain.toml"];

/// Returns the files a copy should take from one source entry: every regular file below a
/// directory, or the entry itself when it is a file.
///
/// The two cases are named because a caller that walked a file would fail, and a caller that
/// treated a directory as one file would copy nothing.
///
/// # Errors
///
/// Returns the reason when a directory below the entry cannot be read.
fn source_files(given: &Utf8Path) -> Result<Vec<Utf8PathBuf>, String> {
    if given.is_dir() {
        return walk_files(given);
    }
    Ok(vec![given.to_owned()])
}

/// Returns every regular file below `directory`, in a stable order.
///
/// Each open directory is its own capability handle, and a child is reached through the handle
/// that opened it, so a walk cannot escape the tree it started in.
///
/// # Errors
///
/// Returns the reason when the directory or a nested path cannot be read.
fn walk_files(directory: &Utf8Path) -> Result<Vec<Utf8PathBuf>, String> {
    let mut files = Vec::new();
    // Each entry pairs the absolute path to open with its path relative to `directory`, so a
    // child inherits the handle its parent opened rather than re-deriving one from a string.
    let mut pending = vec![(directory.to_owned(), Utf8PathBuf::new())];
    while let Some((handle_path, relative)) = pending.pop() {
        let handle = Dir::open_ambient_dir(&handle_path, ambient_authority())
            .map_err(|error| format!("opening {handle_path}: {error}"))?;
        for result in handle
            .entries()
            .map_err(|error| format!("reading directory {handle_path}: {error}"))?
        {
            let entry =
                result.map_err(|error| format!("reading an entry of {handle_path}: {error}"))?;
            let name = entry
                .file_name()
                .map_err(|error| format!("naming an entry of {handle_path}: {error}"))?;
            let path = directory.join(relative.join(&name));
            let kind = entry
                .file_type()
                .map_err(|error| format!("statting {path}: {error}"))?;
            if kind.is_dir() {
                pending.push((path.clone(), relative.join(&name)));
            } else if kind.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Opens the directory holding `path` as a capability, so the read it performs cannot reach
/// anywhere that handle does not.
fn open_parent(path: &Utf8Path) -> Result<(Dir, String), String> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(format!("{path} has no parent directory to open"));
    };
    let handle = Dir::open_ambient_dir(parent, ambient_authority())
        .map_err(|error| format!("opening {parent}: {error}"))?;
    Ok((handle, name.to_owned()))
}

/// Reads a file through a handle on its own directory.
///
/// # Errors
///
/// Returns the reason when the file cannot be opened or read.
fn read_capable(path: &Utf8Path) -> Result<Vec<u8>, String> {
    let (handle, name) = open_parent(path)?;
    handle
        .read(&name)
        .map_err(|error| format!("reading {path}: {error}"))
}

/// Returns a file's mode through a handle on its own directory.
///
/// # Errors
///
/// Returns the reason when the file cannot be statted.
fn mode_of(path: &Utf8Path) -> Result<u32, String> {
    let (handle, name) = open_parent(path)?;
    handle
        .metadata(&name)
        .map_err(|error| format!("statting {path}: {error}"))
        .map(|metadata| metadata.permissions().mode())
}

/// Copies the repository's scripts, the pins they read, and its toolchain file into `sandbox`,
/// returning the copied scripts directory's absolute path.
///
/// A script locates the repository from its own directory, so a copy under the sandbox makes that
/// root the repository: the pin files resolve inside the sandbox, and a case may rewrite a copy to
/// prove something about the script's text without touching the tree under review.
///
/// ```text
/// copy_into(&sandbox, "/repo")  -> Ok("/tmp/sandbox/scripts")  with /tmp/sandbox/rust-toolchain.toml
/// ```
///
/// # Errors
///
/// Returns the reason when a source file cannot be read, or a copy cannot be written or given its
/// mode.
pub fn copy_into(sandbox: &Sandbox, source: &Utf8Path) -> Result<Utf8PathBuf, String> {
    for relative in SOURCES {
        for entry in source_files(&source.join(relative))? {
            copy_file(sandbox, source, &entry)?;
        }
    }
    Ok(sandbox.root().join("scripts"))
}

/// Copies one file into `sandbox` at its path relative to `source`, keeping its mode.
///
/// The mode matters because a script copied without its executable bit cannot run, which would
/// look like a failing script rather than a broken fixture.
fn copy_file(sandbox: &Sandbox, source: &Utf8Path, entry: &Utf8Path) -> Result<(), String> {
    let inside = entry
        .strip_prefix(source)
        .map_err(|error| format!("locating {entry}: {error}"))?;
    let contents = read_capable(entry)?;
    let mode = mode_of(entry)?;
    if let Some(parent) = inside.parent() {
        sandbox.make_dir(parent)?;
    }
    sandbox.write_file(inside, &contents)?;
    sandbox.set_mode(inside, mode)
}
