//! Durable recovery journals for ownership ledgers.
//!
//! Each allocation owns one JSON journal under the state directory. The
//! journal is written before the first provider request and rewritten after
//! every allocation, using a same-directory temporary file and an atomic
//! rename, so an interrupted process leaves either the previous complete
//! snapshot or the new one. `mriya recover` replays the journals that remain.

use std::{io, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ownership::{
    AllocationScope,
    AllocationTag,
    LedgerEntry,
    LedgerSink,
    LedgerSnapshot,
    ResourceLedger,
};

/// Environment variable that overrides the base state directory.
pub const STATE_DIR_ENV: &str = "MRIYA_STATE_DIR";
/// Environment variable that overrides the XDG state home.
const XDG_STATE_HOME_ENV: &str = "XDG_STATE_HOME";
/// Environment variable naming the home directory used when XDG is unset.
const HOME_ENV: &str = "HOME";
/// Application directory created beneath the state base directory.
const APP_DIR: &str = "mriya";
/// Directory that holds one journal per allocation.
const RUNS_DIR: &str = "runs";
/// Journal schema version written by this build.
const JOURNAL_VERSION: u32 = 1;
/// File extension used for journals, which also filters temporary files out of
/// listings.
const JOURNAL_EXTENSION: &str = "json";

/// Errors raised while reading or writing recovery journals.
#[derive(Debug, Error)]
pub enum JournalError {
    /// Raised when neither the state override nor a home directory is set.
    #[error("cannot determine the state directory: set MRIYA_STATE_DIR, XDG_STATE_HOME, or HOME")]
    NoStateDir,
    /// Raised when a journal path cannot be read or written.
    #[error("failed to access {path}: {message}")]
    Io {
        /// Path that failed.
        path: Utf8PathBuf,
        /// Human-readable failure description.
        message: String,
    },
    /// Raised when a journal is not valid JSON or has an unsupported schema.
    #[error("failed to parse journal {path}: {message}")]
    Parse {
        /// Path that failed to parse.
        path: Utf8PathBuf,
        /// Human-readable failure description.
        message: String,
    },
}

impl JournalError {
    /// Builds an I/O failure naming `path`.
    fn io(path: &Utf8Path, err: &io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        }
    }
}

/// Contents of one journal file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JournalRecord {
    /// Schema version of this record.
    pub version: u32,
    /// Correlation tag shared by the recorded resources.
    pub allocation: AllocationTag,
    /// Zone and project that bound the records.
    pub scope: AllocationScope,
    /// Resources recorded when the snapshot was taken.
    pub entries: Vec<LedgerEntry>,
    /// Whether an allocation may exist without a recorded identifier.
    ///
    /// A journal with this set must not be removed until `mriya recover` has
    /// reconciled the tag, because the allocation may still appear.
    #[serde(default)]
    pub outcome_unknown: bool,
    /// Process that owns this journal, when it could be identified.
    ///
    /// Recovery refuses to clean a journal whose owner is still running: a live
    /// `mriya run` rewrites its journal after each allocation, and cleaning it
    /// from underneath would delete the resources the run is using.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ProcessIdentity>,
}

impl JournalRecord {
    /// Builds a record from a ledger snapshot.
    #[must_use]
    fn from_snapshot(snapshot: &LedgerSnapshot) -> Self {
        Self {
            version: JOURNAL_VERSION,
            allocation: snapshot.allocation.clone(),
            scope: snapshot.scope.clone(),
            entries: snapshot.entries.clone(),
            outcome_unknown: snapshot.outcome_unknown,
            owner: None,
        }
    }

    /// Builds a ledger that persists through `journal`, preloaded with this
    /// record's entries.
    #[must_use]
    pub fn into_ledger(self, journal: Arc<dyn LedgerSink>) -> ResourceLedger {
        let unknown = self.outcome_unknown;
        let ledger = ResourceLedger::with_tag(self.scope, self.allocation, Some(journal));
        ledger.replace_entries(self.entries);
        if unknown {
            ledger.mark_outcome_unknown();
        }
        ledger
    }
}

/// Process that owns a journal.
mod identity;
/// Reading, writing, and removing journal files.
mod store;

pub use identity::ProcessIdentity;
pub use store::{JournalStore, RecoveryJournal};

#[cfg(test)]
mod tests;
