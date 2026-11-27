//! A throwaway `PATH` for the cache-device scripts.
//!
//! The scripts decide what to format and mount, so the suite runs them against
//! fake utilities rather than asserting on text alone. This sandbox owns the
//! `bin` directory used as the whole `PATH`, a writable root for the fake
//! utilities' state files, and one command per script execution.

use std::{io, process::Command};

use camino::Utf8PathBuf;
use cap_std::{
    ambient_authority,
    fs::{Permissions, PermissionsExt},
    fs_utf8::Dir,
};
use mockable::{DefaultEnv, Env};
use tempfile::TempDir;

/// Directory under the sandbox root holding the fake utilities.
const BIN: &str = "bin";

/// A temporary root holding a `bin` directory used as the whole `PATH`.
pub struct Sandbox {
    _root: TempDir,
    root_path: Utf8PathBuf,
    dir: Dir,
}

impl Sandbox {
    /// Creates an empty sandbox with its `bin` directory.
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
        dir.create_dir(BIN)
            .map_err(|error| format!("creating {BIN}: {error}"))?;
        Ok(Self {
            _root: root,
            root_path,
            dir,
        })
    }

    /// Returns the sandbox's root as a string, for embedding in fake utilities.
    pub fn root(&self) -> &str { self.root_path.as_str() }

    /// Exposes a host tool in the sandbox's `bin` through a one-line wrapper,
    /// so a script can use that tool and nothing else.
    ///
    /// # Errors
    ///
    /// Returns the reason when the tool is missing from the host or its wrapper
    /// cannot be written.
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
    pub fn write_tool(&self, name: &str, body: impl AsRef<str>) -> Result<(), String> {
        let bash = find_on_path("bash")?;
        let path = format!("{BIN}/{name}");
        self.dir
            .write(&path, format!("#!{bash}\n{}\n", body.as_ref()))
            .map_err(|error| format!("writing {name}: {error}"))?;
        self.dir
            .set_permissions(&path, Permissions::from_mode(0o755))
            .map_err(|error| format!("making {name} executable: {error}"))
    }

    /// Writes a plain state file under the sandbox's root.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file cannot be written.
    pub fn write_file(&self, name: &str, contents: &[u8]) -> Result<(), String> {
        self.dir
            .write(name, contents)
            .map_err(|error| format!("writing {name}: {error}"))
    }

    /// Reads a file under the sandbox's root, or `None` when it is absent.
    ///
    /// # Errors
    ///
    /// Returns the reason when the file exists but cannot be read.
    pub fn read(&self, name: &str) -> Result<Option<String>, String> {
        match self.dir.read_to_string(name) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("reading {name}: {error}")),
        }
    }

    /// Returns a command that sees only the sandbox: its `bin` is the whole
    /// `PATH`, and `HOME` is the sandbox root.
    ///
    /// # Errors
    ///
    /// Returns the reason when `sh` cannot be found on the host.
    pub fn command(&self) -> Result<Command, String> {
        let mut command = Command::new(find_on_path("sh")?);
        command
            .env_clear()
            .env("PATH", self.root_path.join(BIN))
            .env("HOME", self.root_path.as_str());
        Ok(command)
    }
}

/// Looks a program up on the host's `PATH`.
///
/// # Errors
///
/// Returns the reason when `PATH` is unset or cannot be decoded, or when the
/// program is in no `PATH` directory.
fn find_on_path(name: &str) -> Result<Utf8PathBuf, String> {
    let path = match DefaultEnv.raw("PATH") {
        Ok(path) => path,
        Err(std::env::VarError::NotPresent) => return Err(String::from("PATH is not set")),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(String::from(
                "PATH is set to a value that is not valid UTF-8",
            ));
        }
    };
    std::env::split_paths(&path)
        .filter_map(|entry| Utf8PathBuf::try_from(entry).ok())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("`{name}` is not on PATH, so the sandbox cannot link it"))
}
