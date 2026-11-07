//! A throwaway directory standing in for the host's `PATH`, so the build-tools scripts and `make` can
//! run against exactly the tools a test chooses: a pinned `mold`, a drifted one, none, a compiler that
//! cannot link with it. Files go through `cap_std`, so the sandbox cannot reach outside its own root.

use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::{
    ambient_authority,
    fs::{Permissions, PermissionsExt},
    fs_utf8::Dir,
};
use mockable::{DefaultEnv, Env};
use tempfile::TempDir;

/// Where the sandbox keeps tools and installed files, relative to its root.
const BIN: &str = "bin";
const PREFIX: &str = "prefix";
/// The file the fake `probe-cargo` and `rustup` append their arguments to, one call per line.
pub const CARGO_LOG: &str = "cargo.log";

/// A temporary root holding a `bin` directory used as `PATH` and a `prefix` the installer writes to.
pub struct Sandbox {
    _root: TempDir,
    root_path: Utf8PathBuf,
    dir: Dir,
}

/// How many symlinks a lookup follows before it calls the chain a loop.
const MAX_LINK_DEPTH: usize = 16;

/// Returns whether `directory/name` is, or links through any chain of symlinks to, a regular file.
///
/// A capability directory refuses to follow a symlink whose target lies outside it, and a host's
/// tools are exactly that: `/usr/bin/env` may be a link into a multicall binary elsewhere, or into
/// `/etc/alternatives`. So each link is read, not followed, and its target is checked in the target's
/// own parent directory. A dangling link, a loop, or a link to a directory is not a tool.
///
/// ```text
/// is_tool(d, "env") -> true   when d/env -> /usr/lib/cargo/bin/coreutils/env, a regular file
/// is_tool(d, "gone") -> false when d/gone -> ../nowhere
/// ```
pub fn is_tool(directory: &Utf8Path, name: &str) -> bool {
    let mut current = directory.join(name);
    for _ in 0..MAX_LINK_DEPTH {
        let (Some(parent), Some(file)) = (current.parent(), current.file_name()) else {
            return false;
        };
        let Ok(opened) = Dir::open_ambient_dir(parent, ambient_authority()) else {
            return false;
        };
        let Ok(metadata) = opened.symlink_metadata(file) else {
            return false;
        };
        if metadata.is_file() {
            return true;
        }
        if !metadata.is_symlink() {
            return false;
        }
        let Ok(target) = opened.read_link_contents(file) else {
            return false;
        };
        current = if target.is_absolute() {
            target
        } else {
            parent.join(target)
        };
    }
    false
}

/// Looks a program up on the host's `PATH`.
///
/// # Errors
///
/// Returns the reason when the program is on no `PATH` directory.
pub fn find_on_path(name: &str) -> Result<Utf8PathBuf, String> {
    let path = DefaultEnv.string("PATH").ok_or("PATH is not set")?;
    std::env::split_paths(&path)
        .filter_map(|entry| Utf8PathBuf::try_from(entry).ok())
        .find(|directory| is_tool(directory, name))
        .map(|directory| directory.join(name))
        .ok_or_else(|| format!("`{name}` is not on PATH, so the sandbox cannot link it"))
}

impl Sandbox {
    /// Creates an empty sandbox with its `bin` and `prefix` directories.
    ///
    /// # Errors
    ///
    /// Returns the reason when the temporary directory cannot be made.
    pub fn new() -> Result<Self, String> {
        let root = TempDir::new().map_err(|error| format!("making the sandbox: {error}"))?;
        let root_path = Utf8PathBuf::try_from(root.path().to_path_buf())
            .map_err(|error| format!("the sandbox path is not UTF-8: {error}"))?;
        let dir = Dir::open_ambient_dir(&root_path, ambient_authority())
            .map_err(|error| format!("opening the sandbox: {error}"))?;
        for name in [BIN, PREFIX] {
            dir.create_dir(name)
                .map_err(|error| format!("creating {name}: {error}"))?;
        }
        Ok(Self {
            _root: root,
            root_path,
            dir,
        })
    }

    /// Returns the sandbox's root.
    pub fn root(&self) -> &Utf8Path {
        &self.root_path
    }

    /// Returns the directory the installer unpacks into.
    pub fn prefix(&self) -> Utf8PathBuf {
        self.root_path.join(PREFIX)
    }

    /// Exposes host tools in the sandbox's `bin` through one-line wrappers, so a script can use them and
    /// nothing else. A wrapper rather than a symlink: a capability directory refuses a link whose target
    /// lies outside it.
    ///
    /// # Errors
    ///
    /// Returns the reason when a tool is missing from the host or its wrapper cannot be written.
    pub fn link_tools(&self, names: &[&str]) -> Result<(), String> {
        for name in names {
            let target = find_on_path(name)?;
            self.write_tool(name, &format!("exec '{target}' \"$@\""))?;
        }
        Ok(())
    }

    /// Writes an executable shell script into the sandbox's `bin`.
    ///
    /// # Errors
    ///
    /// Returns the reason when the script cannot be written or made executable.
    pub fn write_tool(&self, name: &str, body: &str) -> Result<(), String> {
        let bash = find_on_path("bash")?;
        let path = format!("{BIN}/{name}");
        self.dir
            .write(&path, format!("#!{bash}\n{body}\n"))
            .map_err(|error| format!("writing {name}: {error}"))?;
        self.dir
            .set_permissions(&path, Permissions::from_mode(0o755))
            .map_err(|error| format!("making {name} executable: {error}"))
    }

    /// Creates a directory, and its parents, under the sandbox's root.
    ///
    /// # Errors
    ///
    /// Returns the reason when the directory cannot be made.
    pub fn make_dir(&self, relative: &str) -> Result<(), String> {
        self.dir
            .create_dir_all(relative)
            .map_err(|error| format!("creating {relative}: {error}"))
    }

    /// Creates a symlink whose contents are `target`, verbatim, at `relative` under the root.
    ///
    /// # Errors
    ///
    /// Returns the reason when the link cannot be made.
    pub fn link(&self, relative: &str, target: &str) -> Result<(), String> {
        self.dir
            .symlink_contents(target, relative)
            .map_err(|error| format!("linking {relative}: {error}"))
    }

    /// Writes a plain file under the sandbox's root.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file cannot be written.
    pub fn write_file(&self, relative: &str, contents: &[u8]) -> Result<(), String> {
        self.dir
            .write(relative, contents)
            .map_err(|error| format!("writing {relative}: {error}"))
    }

    /// Reads a file under the sandbox's root, or `None` when it does not exist.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file exists but cannot be read.
    pub fn read(&self, relative: &str) -> Result<Option<String>, String> {
        if !self.dir.exists(relative) {
            return Ok(None);
        }
        self.dir
            .read_to_string(relative)
            .map(Some)
            .map_err(|error| format!("reading {relative}: {error}"))
    }

    /// Returns a command that sees only the sandbox: its `bin` is the whole `PATH`, `HOME` is the root,
    /// the installer's prefix is the sandbox's, and the fake Cargo logs to [`CARGO_LOG`].
    ///
    /// # Errors
    ///
    /// Returns the reason when the program cannot be found on the host.
    pub fn command(&self, program: &str) -> Result<Command, String> {
        let mut command = Command::new(find_on_path(program)?);
        command
            .env_clear()
            .env("PATH", self.root_path.join(BIN))
            .env("HOME", &self.root_path)
            .env("BUILD_TOOLS_PREFIX", self.prefix())
            .env("CARGO_LOG", self.root_path.join(CARGO_LOG))
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        Ok(command)
    }
}
