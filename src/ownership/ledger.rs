//! The ownership ledger: a shared, persistable record of provider resources.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camino::Utf8PathBuf;

use super::{
    sink::{LedgerError, LedgerPersistError, LedgerSink},
    types::{
        AllocationScope,
        AllocationTag,
        Disposition,
        LedgerEntry,
        LedgerSnapshot,
        ResourceKind,
        ResourceRef,
    },
};

/// Mutable ledger contents guarded by the ledger's mutex.
#[derive(Debug)]
struct LedgerState {
    /// Correlation tag shared by every recorded resource.
    tag: AllocationTag,
    /// Zone and project that bound the records.
    scope: AllocationScope,
    /// Resources recorded so far.
    entries: Vec<LedgerEntry>,
    /// Non-fatal persistence failures collected while recording.
    warnings: Vec<String>,
    /// Whether a provider allocation may exist without a recorded identifier.
    outcome_unknown: bool,
}

impl LedgerState {
    /// Replaces or appends an entry for `resource`.
    fn upsert(&mut self, resource: ResourceRef, disposition: Disposition) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.resource.matches(resource.kind(), resource.id()))
        {
            existing.resource = resource;
            existing.disposition = disposition;
            return;
        }
        self.entries.push(LedgerEntry::new(resource, disposition));
    }

    /// Builds the snapshot persisted by the sink.
    fn snapshot(&self) -> LedgerSnapshot {
        LedgerSnapshot {
            allocation: self.tag.clone(),
            scope: self.scope.clone(),
            entries: self.entries.clone(),
            outcome_unknown: self.outcome_unknown,
        }
    }
}

/// Cloneable, thread-safe record of the resources one allocation owns.
#[derive(Clone, Debug)]
pub struct ResourceLedger {
    /// Shared ledger contents.
    state: Arc<Mutex<LedgerState>>,
    /// Optional durable sink, invoked on every mutation.
    sink: Option<Arc<dyn LedgerSink>>,
}

impl ResourceLedger {
    /// Creates an in-memory ledger with a freshly generated allocation tag.
    #[must_use]
    pub fn in_memory(scope: AllocationScope) -> Self {
        Self::with_tag(scope, AllocationTag::generate(), None)
    }

    /// Creates a ledger that persists every mutation through `sink`.
    ///
    /// No initial write is performed: call [`ResourceLedger::persist_initial`]
    /// before the first provider request so a create that is interrupted
    /// mid-flight still has a journal naming its tag.
    ///
    /// # Parameters
    ///
    /// - `scope`: the zone and project every recorded resource belongs to.
    /// - `sink`: durable destination written on every mutation.
    ///
    /// # Returns
    ///
    /// An empty ledger with a freshly generated allocation tag.
    #[must_use]
    pub fn with_sink(scope: AllocationScope, sink: Arc<dyn LedgerSink>) -> Self {
        Self::with_tag(scope, AllocationTag::generate(), Some(sink))
    }

    /// Creates a ledger with an explicit tag, used when recovering a journal.
    #[must_use]
    pub fn with_tag(
        scope: AllocationScope,
        tag: AllocationTag,
        sink: Option<Arc<dyn LedgerSink>>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(LedgerState {
                tag,
                scope,
                entries: Vec::new(),
                warnings: Vec::new(),
                outcome_unknown: false,
            })),
            sink,
        }
    }

    /// Returns the allocation tag shared by this ledger's resources.
    #[must_use]
    pub fn tag(&self) -> AllocationTag { self.lock().tag.clone() }

    /// Returns the zone and project this ledger is scoped to.
    #[must_use]
    pub fn scope(&self) -> AllocationScope { self.lock().scope.clone() }

    /// Returns the journal path backing this ledger, when it has one.
    #[must_use]
    pub fn journal_path(&self) -> Option<Utf8PathBuf> {
        self.sink.as_ref().and_then(|sink| sink.location())
    }

    /// Writes the current (possibly empty) snapshot before any provider
    /// request is made.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerPersistError`] when the initial snapshot cannot be
    /// written; the caller must not contact the provider in that case.
    pub fn persist_initial(&self) -> Result<(), LedgerPersistError> {
        // The lock is held across the write so a concurrent mutation cannot
        // publish a newer snapshot that this initial write then overwrites.
        let state = self.lock();
        self.persist(&state.snapshot())
    }

    /// Records `resource` with `disposition`, replacing an earlier entry for
    /// the same resource.
    ///
    /// A persistence failure does not lose the in-memory record: cleanup can
    /// still act on it during this process, and the failure is kept in
    /// [`ResourceLedger::warnings`] so the eventual report can mention it.
    pub fn record(&self, resource: ResourceRef, disposition: Disposition) {
        // The snapshot is taken and written while the lock is held, so two
        // clones cannot publish an older snapshot after a newer one. The lock
        // covers one small file write; correctness beats that contention.
        let mut state = self.lock();
        state.upsert(resource, disposition);
        let snapshot = state.snapshot();
        if let Err(err) = self.persist(&snapshot) {
            state.warnings.push(err.message);
        }
    }

    /// Records `resource` and reports whether the change reached the journal.
    ///
    /// Unlike [`ResourceLedger::record`], a persistence failure is returned
    /// rather than collected, for a caller whose next step is unsafe unless
    /// the allocation is durably recorded.
    ///
    /// # Parameters
    ///
    /// - `resource`: the allocation to record.
    /// - `disposition`: what cleanup may do with it.
    ///
    /// # Returns
    ///
    /// `Ok(())` once memory and the journal agree.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::Persist`] when the journal write fails; the
    /// in-memory record is still updated so this process can clean it up.
    pub fn record_durable(
        &self,
        resource: ResourceRef,
        disposition: Disposition,
    ) -> Result<(), LedgerError> {
        let mut state = self.lock();
        state.upsert(resource, disposition);
        let snapshot = state.snapshot();
        self.persist(&snapshot).map_err(|err| LedgerError::Persist {
            message: err.message,
        })
    }

    /// Changes the disposition recorded for an already-recorded resource.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::NotRecorded`] when no entry names `kind` and
    /// `id`, and [`LedgerError::Persist`] when the change could not be written
    /// to the journal. The in-memory disposition is updated even then, so this
    /// process still honours it.
    pub fn promote(
        &self,
        kind: ResourceKind,
        id: &str,
        disposition: Disposition,
    ) -> Result<(), LedgerError> {
        let mut state = self.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.resource.matches(kind, id))
        else {
            return Err(LedgerError::NotRecorded {
                kind,
                id: id.to_owned(),
            });
        };
        entry.disposition = disposition;
        let snapshot = state.snapshot();
        self.persist(&snapshot).map_err(|err| LedgerError::Persist {
            message: err.message,
        })
    }

    /// Replaces the recorded entries with `entries`.
    ///
    /// Recovery uses this after rediscovering resources from provider
    /// metadata, so a journal that never learned an identifier is corrected
    /// before cleanup runs.
    pub fn replace_entries(&self, entries: Vec<LedgerEntry>) {
        let mut state = self.lock();
        state.entries = entries;
        let snapshot = state.snapshot();
        if let Err(err) = self.persist(&snapshot) {
            state.warnings.push(err.message);
        }
    }

    /// Restores a recovered snapshot in one state update.
    ///
    /// Entries and the unknown-outcome flag are written together, so a journal
    /// can never be left with the entries restored but the flag lost: that
    /// combination would skip the reconciliation an unknown outcome needs.
    ///
    /// # Parameters
    ///
    /// - `entries`: resources recorded by the journal.
    /// - `outcome_unknown`: whether the journal recorded an unresolved create.
    ///
    /// # Returns
    ///
    /// Nothing; a persistence failure is kept in [`ResourceLedger::warnings`].
    pub fn restore(&self, entries: Vec<LedgerEntry>, outcome_unknown: bool) {
        let mut state = self.lock();
        state.entries = entries;
        state.outcome_unknown = outcome_unknown;
        let snapshot = state.snapshot();
        if let Err(err) = self.persist(&snapshot) {
            state.warnings.push(err.message);
        }
    }

    /// Returns a copy of every recorded entry.
    #[must_use]
    pub fn entries(&self) -> Vec<LedgerEntry> { self.lock().entries.clone() }

    /// Returns the entry for `kind` and `id`, when one exists.
    #[must_use]
    pub fn find(&self, kind: ResourceKind, id: &str) -> Option<LedgerEntry> {
        self.lock()
            .entries
            .iter()
            .find(|entry| entry.resource.matches(kind, id))
            .cloned()
    }

    /// Returns every entry of `kind`.
    #[must_use]
    pub fn entries_of(&self, kind: ResourceKind) -> Vec<LedgerEntry> {
        self.lock()
            .entries
            .iter()
            .filter(|entry| entry.resource.kind() == kind)
            .cloned()
            .collect()
    }

    /// Returns every entry with `disposition`.
    #[must_use]
    pub fn entries_with(&self, disposition: Disposition) -> Vec<LedgerEntry> {
        self.lock()
            .entries
            .iter()
            .filter(|entry| entry.disposition == disposition)
            .cloned()
            .collect()
    }

    /// Returns the non-fatal persistence failures collected so far.
    ///
    /// These are warnings rather than errors: [`ResourceLedger::record`] has
    /// already updated memory, so cleanup in this process still sees the
    /// resource. [`ResourceLedger::promote`] reports the same condition as an
    /// error instead, because a caller that must not continue relies on it.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> { self.lock().warnings.clone() }

    /// Removes the durable journal after verified cleanup.
    ///
    /// The removal is skipped while an allocation outcome is unknown, because
    /// the tag is then the only durable correlation with the provider. A
    /// successful return therefore does not prove the file is gone; use
    /// [`ResourceLedger::has_unknown_outcome`] or
    /// [`ResourceLedger::journal_path`] when that distinction matters.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerPersistError`] when the journal exists but cannot be
    /// removed; the caller must then report the journal path so `mriya
    /// recover` can find it.
    pub fn clear_journal(&self) -> Result<(), LedgerPersistError> {
        if self.lock().outcome_unknown {
            return Ok(());
        }
        self.sink.as_ref().map_or(Ok(()), |sink| sink.clear())
    }

    /// Marks that a provider allocation may exist without a recorded
    /// identifier, and persists that fact.
    ///
    /// The flag survives a restart because it is part of the journal record:
    /// `mriya recover` must know that the journal cannot be cleared until the
    /// tag has been reconciled.
    pub fn mark_outcome_unknown(&self) {
        let mut state = self.lock();
        state.outcome_unknown = true;
        let snapshot = state.snapshot();
        if let Err(err) = self.persist(&snapshot) {
            state.warnings.push(err.message);
        }
    }

    /// Clears the unknown-outcome flag after a verified reconciliation.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerPersistError`] when the updated record cannot be
    /// written; the journal must then be kept.
    pub fn resolve_outcome(&self) -> Result<(), LedgerPersistError> {
        let mut state = self.lock();
        state.outcome_unknown = false;
        let snapshot = state.snapshot();
        self.persist(&snapshot)
    }

    /// Reports whether an allocation outcome is unknown.
    #[must_use]
    pub fn has_unknown_outcome(&self) -> bool { self.lock().outcome_unknown }

    /// Persists `snapshot` through the sink, if one is configured.
    fn persist(&self, snapshot: &LedgerSnapshot) -> Result<(), LedgerPersistError> {
        self.sink
            .as_ref()
            .map_or(Ok(()), |sink| sink.persist(snapshot))
    }

    /// Locks the ledger, recovering the contents if a panic poisoned the lock.
    ///
    /// A poisoned ledger still holds the authoritative record of what was
    /// allocated, so discarding it would leak resources. Recovering the guard
    /// keeps cleanup able to act on it.
    fn lock(&self) -> MutexGuard<'_, LedgerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
