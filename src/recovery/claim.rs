//! Exclusive claims on recovery journals.
//!
//! A claim is a lock file created beside the journal. Holding it across the
//! owner check and the replay is what stops two `mriya recover` processes from
//! replaying the same journal at once.

use std::io;

use camino::{Utf8Path, Utf8PathBuf};
#[cfg(unix)]
use cap_std::fs_utf8::OpenOptionsExt as _;
use cap_std::{
    ambient_authority,
    fs_utf8::{Dir, OpenOptions},
};

use super::{
    JournalError,
    JournalStore,
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
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match dir.open_with(&lock_name, &options) {
            Ok(file) => {
                drop(file);
                Ok(Some(JournalClaim {
                    path: parent.join(&lock_name),
                }))
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(None),
            Err(err) => Err(JournalError::io(&parent.join(&lock_name), &err)),
        }
    }
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
