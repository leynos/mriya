//! Durable storage for recovery journals.
//!
//! Journals are replaced atomically through a same-directory temporary file so
//! an interrupted write leaves either the previous snapshot or the new one,
//! never a partial file.

use std::{
    fmt,
    io::{self, Write},
    sync::Arc,
};

use camino::{Utf8Path, Utf8PathBuf};
#[cfg(unix)]
use cap_std::fs_utf8::PermissionsExt;
use cap_std::{
    ambient_authority,
    fs_utf8::{Dir, OpenOptions},
};

use super::{
    APP_DIR,
    HOME_ENV,
    JOURNAL_EXTENSION,
    JOURNAL_VERSION,
    JournalError,
    JournalRecord,
    ProcessIdentity,
    RUNS_DIR,
    STATE_DIR_ENV,
    XDG_STATE_HOME_ENV,
};
use crate::ownership::{AllocationTag, LedgerPersistError, LedgerSink, LedgerSnapshot};

/// Directory that holds recovery journals.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalStore {
    /// Absolute path to the journal directory.
    root: Utf8PathBuf,
}

impl JournalStore {
    /// Resolves the journal directory from the injected environment.
    ///
    /// The base directory is `MRIYA_STATE_DIR`, then `XDG_STATE_HOME`, then
    /// `$HOME/.local/state`; journals live in `mriya/runs` beneath it.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::NoStateDir`] when none of the three variables
    /// yields a non-blank directory.
    pub fn from_env(env: &impl mockable::Env) -> Result<Self, JournalError> {
        let base = first_non_blank(env, &[STATE_DIR_ENV, XDG_STATE_HOME_ENV])
            .or_else(|| {
                first_non_blank(env, &[HOME_ENV]).map(|home| format!("{home}/.local/state"))
            })
            .ok_or(JournalError::NoStateDir)?;
        Ok(Self::at_root(
            Utf8PathBuf::from(base).join(APP_DIR).join(RUNS_DIR),
        ))
    }

    /// Uses `root` as the journal directory.
    #[must_use]
    pub const fn at_root(root: Utf8PathBuf) -> Self { Self { root } }

    /// Returns the journal directory.
    #[must_use]
    pub fn root(&self) -> &Utf8Path { self.root.as_path() }

    /// Returns the journal path for `tag`.
    #[must_use]
    pub fn journal_path(&self, tag: &AllocationTag) -> Utf8PathBuf {
        self.root.join(format!("{tag}.{JOURNAL_EXTENSION}"))
    }

    /// Builds the journal handle for `tag` without writing anything yet.
    #[must_use]
    pub fn handle(&self, tag: &AllocationTag) -> RecoveryJournal {
        RecoveryJournal {
            path: self.journal_path(tag),
            owner: ProcessIdentity::current(),
        }
    }

    /// Lists every journal file, sorted, treating a missing directory as none.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] when the directory exists but cannot be
    /// read.
    pub fn journals(&self) -> Result<Vec<Utf8PathBuf>, JournalError> {
        let entries = match Dir::open_ambient_dir(&self.root, ambient_authority()) {
            Ok(dir) => dir,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(JournalError::io(&self.root, &err)),
        };

        let mut paths = Vec::new();
        for entry_result in entries
            .read_dir(".")
            .map_err(|err| JournalError::io(&self.root, &err))?
        {
            let dir_entry = entry_result.map_err(|err| JournalError::io(&self.root, &err))?;
            let file_type = dir_entry
                .file_type()
                .map_err(|err| JournalError::io(&self.root, &err))?;
            if !file_type.is_file() {
                continue;
            }
            let name = dir_entry
                .file_name()
                .map_err(|err| JournalError::io(&self.root, &err))?;
            if name.ends_with(&format!(".{JOURNAL_EXTENSION}")) {
                paths.push(self.root.join(name));
            }
        }
        paths.sort();
        Ok(paths)
    }

    /// Reads and validates the journal at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] when the file cannot be read and
    /// [`JournalError::Parse`] when it is not a supported journal.
    pub fn load(&self, path: &Utf8Path) -> Result<JournalRecord, JournalError> {
        let dir = open_parent(path)?;
        let file_name = file_name(path)?;
        let contents = dir
            .read_to_string(file_name)
            .map_err(|err| JournalError::io(path, &err))?;
        let record: JournalRecord =
            serde_json::from_str(&contents).map_err(|err| JournalError::Parse {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;
        if record.version != JOURNAL_VERSION {
            return Err(JournalError::Parse {
                path: path.to_path_buf(),
                message: format!(
                    "unsupported journal version {}; expected {JOURNAL_VERSION}",
                    record.version
                ),
            });
        }
        Ok(record)
    }

    /// Reads `path` and returns the record together with its journal handle.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`JournalStore::load`].
    pub fn open(
        &self,
        path: &Utf8Path,
    ) -> Result<(JournalRecord, Arc<dyn LedgerSink>), JournalError> {
        let record = self.load(path)?;
        let journal: Arc<dyn LedgerSink> = Arc::new(RecoveryJournal {
            path: path.to_path_buf(),
            owner: ProcessIdentity::current(),
        });
        Ok((record, journal))
    }

    /// Removes `path`, treating an already-absent journal as success.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] when the file exists but cannot be
    /// removed.
    pub fn remove(&self, path: &Utf8Path) -> Result<(), JournalError> {
        let dir = open_parent(path)?;
        match dir.remove_file(file_name(path)?) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(JournalError::io(path, &err)),
        }
    }
}

/// Opens the directory that contains `path`.
fn open_parent(path: &Utf8Path) -> Result<Dir, JournalError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .unwrap_or_else(|| Utf8Path::new("."));
    Dir::open_ambient_dir(parent, ambient_authority()).map_err(|err| JournalError::io(parent, &err))
}

/// Opens the directory that contains `path`, reporting `None` when it is
/// absent.
///
/// A removal against a directory that does not exist has already reached its
/// postcondition: the journal is not there.
fn open_parent_if_present(path: &Utf8Path) -> Result<Option<Dir>, JournalError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .unwrap_or_else(|| Utf8Path::new("."));
    match Dir::open_ambient_dir(parent, ambient_authority()) {
        Ok(dir) => Ok(Some(dir)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(JournalError::io(parent, &err)),
    }
}

/// One journal file that can persist ledger snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryJournal {
    /// Absolute path of the journal file.
    path: Utf8PathBuf,
    /// Process that owns the journal.
    owner: Option<ProcessIdentity>,
}

impl RecoveryJournal {
    /// Returns the journal file path.
    #[must_use]
    pub fn path(&self) -> &Utf8Path { self.path.as_path() }

    /// Removes the journal file, treating absence as success.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] when the file exists but cannot be
    /// removed.
    pub fn remove(&self) -> Result<(), JournalError> {
        let Some(dir) = open_parent_if_present(&self.path)? else {
            return Ok(());
        };
        match dir.remove_file(file_name(&self.path)?) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(JournalError::io(&self.path, &err)),
        }
    }

    /// Serializes `record` and replaces the journal with it atomically.
    fn write(&self, record: &JournalRecord) -> Result<(), JournalError> {
        let rendered = serde_json::to_string_pretty(record).map_err(|err| JournalError::Parse {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        write_atomic(&self.path, &rendered)
    }
}

impl LedgerSink for RecoveryJournal {
    fn persist(&self, snapshot: &LedgerSnapshot) -> Result<(), LedgerPersistError> {
        self.write(&JournalRecord::from_snapshot(snapshot))
            .map_err(|err| LedgerPersistError::new(err.to_string()))
    }

    fn location(&self) -> Option<Utf8PathBuf> { Some(self.path.clone()) }

    fn clear(&self) -> Result<(), LedgerPersistError> {
        self.remove()
            .map_err(|error| LedgerPersistError::new(error.to_string()))
    }
}

impl fmt::Display for RecoveryJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.path.as_str())
    }
}

/// Returns the first variable among `names` that holds a non-blank value.
///
/// Only the blank check trims; the returned value keeps its original bytes, so
/// a state directory whose name ends in a space still selects that directory.
fn first_non_blank(env: &impl mockable::Env, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| env.string(name).filter(|value| !value.trim().is_empty()))
}

/// Returns the file name of `path`.
fn file_name(path: &Utf8Path) -> Result<&str, JournalError> {
    path.file_name().ok_or_else(|| JournalError::Io {
        path: path.to_path_buf(),
        message: String::from("journal path has no file name"),
    })
}

/// Writes `contents` to a same-directory temporary file, then renames it over
/// `path`.
///
/// The temporary file is created with mode `0600` so the journal, which names
/// billable resources, is never readable by other users, even briefly.
fn write_atomic(path: &Utf8Path, contents: &str) -> Result<(), JournalError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .unwrap_or_else(|| Utf8Path::new("."));
    let name = file_name(path)?;

    Dir::create_ambient_dir_all(parent, ambient_authority())
        .map_err(|err| JournalError::io(parent, &err))?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())
        .map_err(|err| JournalError::io(parent, &err))?;

    let temporary = format!(".{name}.tmp");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use cap_std::fs_utf8::OpenOptionsExt as _;
        options.mode(0o600);
    }

    let mut file = dir
        .open_with(&temporary, &options)
        .map_err(|err| JournalError::io(path, &err))?;
    file.write_all(contents.as_bytes())
        .map_err(|err| JournalError::io(path, &err))?;
    file.sync_all()
        .map_err(|err| JournalError::io(path, &err))?;
    drop(file);

    // A stale temporary file from an earlier interrupted write may carry
    // different permissions; the mode is set again so the rename cannot
    // promote a readable file.
    #[cfg(unix)]
    dir.set_permissions(&temporary, PermissionsExt::from_mode(0o600))
        .map_err(|err| JournalError::io(path, &err))?;
    dir.rename(&temporary, &dir, name)
        .map_err(|err| JournalError::io(path, &err))
}
