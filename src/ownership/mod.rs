//! Thread-safe ownership ledger and its durable persistence port.
//!
//! A [`ResourceLedger`] records each provider allocation as soon as the
//! provider confirms it. Cleanup then works from the ledger rather than from a
//! single instance handle, so a partially provisioned run still knows what it
//! created.
//!
//! Persistence is guaranteed only for the initial write: a failed
//! [`ResourceLedger::persist_initial`] stops the caller before any provider
//! request. Later mutations attempt a write under the ledger lock and keep the
//! in-memory record either way, reporting a failure through
//! [`ResourceLedger::warnings`] or, for
//! [`ResourceLedger::promote`], as an error. A crash after a failed write can
//! therefore lose a record that this process still knows about, which is why
//! `mriya init` refuses to continue when the retention of a formatted volume
//! cannot be persisted.

mod ledger;
mod sink;

pub use ledger::ResourceLedger;
pub use sink::{LedgerError, LedgerPersistError, LedgerSink};
pub use types::{
    ALLOCATION_TAG_PREFIX,
    AllocationScope,
    AllocationTag,
    Disposition,
    LedgerEntry,
    LedgerSnapshot,
    ResourceKind,
    ResourceRef,
    StorageFamily,
};

mod types;

#[cfg(test)]
mod tests;
