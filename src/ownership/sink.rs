//! Durable ledger destinations and the errors they report.

use std::fmt;

use camino::Utf8PathBuf;
use thiserror::Error;

use super::types::{LedgerSnapshot, ResourceKind};

/// Failure returned by a [`LedgerSink`] when it cannot durably record a
/// snapshot.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{message}")]
pub struct LedgerPersistError {
    /// Human-readable description of the persistence failure.
    pub message: String,
}

impl LedgerPersistError {
    /// Wraps a message in a persistence failure.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Durable destination for ledger snapshots.
///
/// The ownership module owns this port; the recovery journal is the adapter
/// that writes it to disk. Keeping the interface here lets cleanup run with an
/// in-memory ledger in tests and with a journal in production.
pub trait LedgerSink: fmt::Debug + Send + Sync {
    /// Writes `snapshot` durably, replacing any earlier snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerPersistError`] when the snapshot cannot be written.
    fn persist(&self, snapshot: &LedgerSnapshot) -> Result<(), LedgerPersistError>;

    /// Returns the journal path backing this sink, when it has one.
    fn location(&self) -> Option<Utf8PathBuf> { None }

    /// Removes the durable record, called only after verified cleanup.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerPersistError`] when the record exists but cannot be
    /// removed.
    fn clear(&self) -> Result<(), LedgerPersistError> { Ok(()) }
}

/// Errors raised by ledger queries and promotions.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum LedgerError {
    /// Raised when a promotion names a resource that was never recorded.
    #[error("{kind} {id} is not recorded in the ownership ledger")]
    NotRecorded {
        /// Kind that was looked up.
        kind: ResourceKind,
        /// Identifier that was looked up.
        id: String,
    },
    /// Raised when a promotion changes memory but cannot be persisted.
    ///
    /// The in-memory disposition is already updated, so cleanup in this
    /// process still preserves the resource; the journal, however, still shows
    /// the previous disposition. A caller for which that difference matters
    /// must not report success.
    #[error("the ledger change could not be persisted: {message}")]
    Persist {
        /// Human-readable persistence failure.
        message: String,
    },
}
