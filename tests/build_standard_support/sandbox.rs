//! A throwaway directory standing in for the host's `PATH`, so the build-tools scripts and `make` can
//! run against exactly the tools a test chooses: a pinned `mold`, a drifted one, none, a compiler that
//! cannot link with it. Files go through `cap_std`, so the sandbox cannot reach outside its own root.

use std::io;
use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::{
    ambient_authority,
    fs::{Permissions, PermissionsExt},
    fs_utf8::Dir,
};
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

/// What one component of a lookup path turned out to be, without following its link.
enum Entry {
    /// A regular file, so the lookup succeeded.
    File,
    /// A link and the path it names, which the lookup still has to judge.
    Link(Utf8PathBuf),
    /// Anything else: a directory or another file type.
    Other,
}

/// Returns whether a failed read means the path is absent rather than unreadable.
///
/// A path below a regular file is `NotADirectory`, which is as absent as a
/// missing parent for a lookup's purposes. Everything else — a permission
/// failure, an I/O error, a symlink loop in the parent — says the filesystem
/// could not answer, which is not the same as answering "no".
fn is_absence(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// Reads one path component and resolves at most one link from it.
///
/// A capability directory refuses to follow a symlink whose target lies outside it, and a host's
/// tools are exactly that: `/usr/bin/env` may be a link into a multicall binary elsewhere, or into
/// `/etc/alternatives`. So the link is read, not followed, and its target is resolved in the
/// target's own parent directory.
///
/// # Errors
///
/// Returns the reason, naming the path, when the filesystem fails for any reason other than the
/// path being absent.
fn entry_at(path: &Utf8Path) -> Result<Option<Entry>, String> {
    let (Some(parent), Some(file)) = (path.parent(), path.file_name()) else {
        return Ok(Some(Entry::Other));
    };
    let unreadable = |error: io::Error| format!("reading {path}: {error}");
    let opened = match Dir::open_ambient_dir(parent, ambient_authority()) {
        Ok(opened) => opened,
        Err(error) if is_absence(&error) => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    let metadata = match opened.symlink_metadata(file) {
        Ok(metadata) => metadata,
        Err(error) if is_absence(&error) => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    if metadata.is_file() {
        return Ok(Some(Entry::File));
    }
    if !metadata.is_symlink() {
        return Ok(Some(Entry::Other));
    }
    let target = match opened.read_link_contents(file) {
        Ok(target) => target,
        Err(error) if is_absence(&error) => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    let resolved = if target.is_absolute() {
        target
    } else {
        parent.join(target)
    };
    Ok(Some(Entry::Link(resolved)))
}

/// Returns whether `directory/name` is, or links through any chain of symlinks to, a regular file.
///
/// A dangling link, a loop, or a link to a directory is not a tool. A path that
/// exists but cannot be read is an error, never an absence: a contract that
/// reported "not a tool" for an unreadable one would pass while proving nothing.
///
/// ```text
/// is_tool(d, "env")  -> Ok(true)   when d/env -> /usr/lib/cargo/bin/coreutils/env, a regular file
/// is_tool(d, "gone") -> Ok(false)  when d/gone -> ../nowhere
/// ```
///
/// # Errors
///
/// Returns the reason when a path cannot be read for a reason other than absence.
pub fn is_tool(directory: &Utf8Path, name: impl AsRef<str>) -> Result<bool, String> {
    let mut current = directory.join(name.as_ref());
    for _ in 0..MAX_LINK_DEPTH {
        match entry_at(&current)? {
            Some(Entry::File) => return Ok(true),
            Some(Entry::Link(target)) => current = target,
            Some(Entry::Other) | None => return Ok(false),
        }
    }
    Ok(false)
}

/// Looks a program up on the host's `PATH`.
///
/// # Errors
///
/// Returns the reason when the program is on no `PATH` directory, or when a `PATH` entry cannot be
/// read for a reason other than absence.
pub fn find_on_path(name: impl AsRef<str>) -> Result<Utf8PathBuf, String> {
    let program = name.as_ref();
    let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
    let directories =
        std::env::split_paths(&path).filter_map(|entry| Utf8PathBuf::try_from(entry).ok());
    for directory in directories {
        if is_tool(&directory, program)? {
            return Ok(directory.join(program));
        }
    }
    Err(format!(
        "`{program}` is not on PATH, so the sandbox cannot link it"
    ))
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
            self.write_tool(name, format!("exec '{target}' \"$@\""))?;
        }
        Ok(())
    }

    /// Writes an executable shell script into the sandbox's `bin`.
    ///
    /// # Errors
    ///
    /// Returns the reason when the script cannot be written or made executable.
    pub fn write_tool(&self, name: impl AsRef<str>, body: impl AsRef<str>) -> Result<(), String> {
        let tool = name.as_ref();
        let script = body.as_ref();
        let bash = find_on_path("bash")?;
        let path = format!("{BIN}/{tool}");
        self.dir
            .write(&path, format!("#!{bash}\n{script}\n"))
            .map_err(|error| format!("writing {tool}: {error}"))?;
        self.dir
            .set_permissions(&path, Permissions::from_mode(0o755))
            .map_err(|error| format!("making {tool} executable: {error}"))
    }

    /// Creates a directory, and its parents, under the sandbox's root.
    ///
    /// # Errors
    ///
    /// Returns the reason when the directory cannot be made.
    pub fn make_dir(&self, relative: impl AsRef<str>) -> Result<(), String> {
        let path = relative.as_ref();
        self.dir
            .create_dir_all(path)
            .map_err(|error| format!("creating {path}: {error}"))
    }

    /// Creates a symlink whose contents are `target`, verbatim, at `relative` under the root.
    ///
    /// # Errors
    ///
    /// Returns the reason when the link cannot be made.
    pub fn link(&self, relative: impl AsRef<str>, target: impl AsRef<str>) -> Result<(), String> {
        let link_path = relative.as_ref();
        let link_target = target.as_ref();
        self.dir
            .symlink_contents(link_target, link_path)
            .map_err(|error| format!("linking {link_path}: {error}"))
    }

    /// Writes a plain file under the sandbox's root.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file cannot be written.
    pub fn write_file(&self, relative: impl AsRef<str>, contents: &[u8]) -> Result<(), String> {
        let path = relative.as_ref();
        self.dir
            .write(path, contents)
            .map_err(|error| format!("writing {path}: {error}"))
    }

    /// Reads a file under the sandbox's root, or `None` when it does not exist.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file exists but cannot be read.
    pub fn read(&self, relative: impl AsRef<str>) -> Result<Option<String>, String> {
        let path = relative.as_ref();
        // Only a genuine missing file is absent. `exists` would turn a permission
        // or I/O failure into a pass, which is how a contract stops proving.
        match self.dir.read_to_string(path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("reading {path}: {error}")),
        }
    }

    /// Makes the sandbox's `uname` answer `Darwin` to `-s`, so both scripts' non-Linux branches run
    /// on any host. Every other query still reaches the host `uname`, so `-m` keeps naming the real
    /// architecture.
    ///
    /// # Errors
    ///
    /// Returns the reason when the host `uname` cannot be found or the stub cannot be written.
    pub fn pretend_darwin(&self) -> Result<(), String> {
        let real = find_on_path("uname")?;
        let body =
            format!("if [ \"${{1-}}\" = '-s' ]; then echo Darwin; else exec '{real}' \"$@\"; fi");
        self.write_tool("uname", body)
    }

    /// Returns a command that sees only the sandbox: its `bin` is the whole `PATH`, `HOME` is the root,
    /// the installer's prefix is the sandbox's, and the fake Cargo logs to [`CARGO_LOG`].
    ///
    /// # Errors
    ///
    /// Returns the reason when the program cannot be found on the host.
    pub fn command(&self, program: impl AsRef<str>) -> Result<Command, String> {
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
