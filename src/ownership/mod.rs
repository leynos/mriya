//! Thread-safe ownership ledger and its durable persistence port.
//!
//! A [`ResourceLedger`] records each provider allocation as soon as the
//! provider confirms it. Cleanup then works from the ledger rather than from a
//! single instance handle, so a partially provisioned run still knows what it
//! created. The ledger is cloneable and shared, and every mutation is
//! persisted through an optional [`LedgerSink`] before the call returns.

mod ledger;

pub use ledger::{LedgerError, LedgerPersistError, LedgerSink, ResourceLedger};
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
