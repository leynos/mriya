//! Exclusive claims on recovery journals.
//!
//! A claim is a lock file created beside the journal. Holding it across the
//! owner check and the replay is what stops two `mriya recover` processes from
//! replaying the same journal at once.

use std::io::{self, Write};

use camino::{Utf8Path, Utf8PathBuf};
#[cfg(unix)]
use cap_std::fs_utf8::OpenOptionsExt as _;
use cap_std::{
    ambient_authority,
    fs_utf8::{Dir, OpenOptions},
};
use serde::{Deserialize, Serialize};

use super::{
    JournalError,
    JournalStore,
    ProcessIdentity,
    store::{file_name, open_parent_if_present},
};

impl JournalStore {
    /// Claims `path` for replay, or reports that another process holds it.
    ///
    /// The claim is a lock file created exclusively beside the journal. It is
    /// what makes the owner check and the replay atomic: without it two
    /// `mriya recover` processes could both read an abandoned owner and then
    /// replay the same journal, with the second acting on resources the first
    /// already deleted.
    ///
    /// # Parameters
    ///
    /// - `path`: journal file to claim.
    ///
    /// # Returns
    ///
    /// `Some(claim)` when this process owns the journal until the claim is
    /// dropped, or `None` when another process already holds it.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] when the claim file cannot be created for
    /// a reason other than an existing claim.
    pub fn claim(&self, path: &Utf8Path) -> Result<Option<JournalClaim>, JournalError> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_str().is_empty())
            .unwrap_or_else(|| Utf8Path::new("."));
        let name = file_name(path)?;
        let lock_name = format!(".{name}.lock");
        Dir::create_ambient_dir_all(parent, ambient_authority())
            .map_err(|err| JournalError::io(parent, &err))?;
        let dir = Dir::open_ambient_dir(parent, ambient_authority())
            .map_err(|err| JournalError::io(parent, &err))?;
        let lock_path = parent.join(&lock_name);
        match create_claim(&dir, &lock_name, &lock_path) {
            Ok(()) => Ok(Some(JournalClaim { path: lock_path })),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Self::reclaim_abandoned(&dir, &lock_name, &lock_path)
            }
            Err(error) => Err(JournalError::io(&lock_path, &error)),
        }
    }

    /// Takes over a claim whose recorded owner has exited.
    ///
    /// Removing and recreating races safely with another reclaimer: only one
    /// `create_new` succeeds, and the loser reports the journal as claimed.
    fn reclaim_abandoned(
        dir: &Dir,
        lock_name: &str,
        lock_path: &Utf8Path,
    ) -> Result<Option<JournalClaim>, JournalError> {
        if claim_owner(dir, lock_name).is_some_and(|owner| owner.is_alive()) {
            return Ok(None);
        }
        match dir.remove_file(lock_name) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(JournalError::io(lock_path, &error)),
        }
        match create_claim(dir, lock_name, lock_path) {
            Ok(()) => Ok(Some(JournalClaim {
                path: lock_path.to_path_buf(),
            })),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(JournalError::io(lock_path, &error)),
        }
    }
}

/// Creates the claim file and records this process as its owner.
fn create_claim(dir: &Dir, name: &str, path: &Utf8Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = dir.open_with(name, &options)?;
    let owner = ClaimOwner {
        owner: ProcessIdentity::current(),
    };
    let rendered = serde_json::to_string(&owner)
        .map_err(|error| io::Error::other(format!("the claim owner must serialize: {error}")))?;
    file.write_all(rendered.as_bytes())?;
    file.sync_all().map_err(|error| {
        io::Error::other(format!(
            "the claim owner could not be written to {path}: {error}"
        ))
    })
}

/// Reads the owner recorded in an existing claim file.
fn claim_owner(dir: &Dir, name: &str) -> Option<ProcessIdentity> {
    let contents = dir.read_to_string(name).ok()?;
    serde_json::from_str::<ClaimOwner>(&contents).ok()?.owner
}

/// Serialized owner of a claim file.
#[derive(Deserialize, Serialize)]
struct ClaimOwner {
    /// Process that created the claim, when it could be identified.
    owner: Option<ProcessIdentity>,
}

/// An exclusive claim on one journal, released when dropped.
#[derive(Debug)]
pub struct JournalClaim {
    /// Path of the lock file.
    path: Utf8PathBuf,
}

impl Drop for JournalClaim {
    fn drop(&mut self) {
        if let Ok(Some(dir)) = open_parent_if_present(&self.path)
            && let Ok(name) = file_name(&self.path)
        {
            drop(dir.remove_file(name));
        }
    }
}
