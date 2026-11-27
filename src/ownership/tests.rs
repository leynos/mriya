//! Unit tests for recording, promotion, and persistence warnings.

use std::sync::{Arc, Mutex};

use camino::Utf8PathBuf;
use rstest::{fixture, rstest};

use super::{
    AllocationScope,
    Disposition,
    LedgerError,
    LedgerPersistError,
    LedgerSink,
    LedgerSnapshot,
    ResourceKind,
    ResourceLedger,
    ResourceRef,
};

#[derive(Debug, Default)]
struct RecordingSink {
    snapshots: Mutex<Vec<LedgerSnapshot>>,
    fail: Mutex<bool>,
}

impl LedgerSink for RecordingSink {
    fn persist(&self, snapshot: &LedgerSnapshot) -> Result<(), LedgerPersistError> {
        if *self.fail.lock().expect("fail flag") {
            return Err(LedgerPersistError::new("disk full"));
        }
        self.snapshots
            .lock()
            .expect("snapshots")
            .push(snapshot.clone());
        Ok(())
    }

    fn location(&self) -> Option<Utf8PathBuf> { Some(Utf8PathBuf::from("/tmp/journal.json")) }
}

#[fixture]
fn scope() -> AllocationScope {
    AllocationScope {
        zone: String::from("fr-par-1"),
        project_id: String::from("project"),
    }
}

#[rstest]
fn record_twice_keeps_one_entry(scope: AllocationScope) {
    let ledger = ResourceLedger::in_memory(scope);
    ledger.record(ResourceRef::server("srv-1", "fr-par-1"), Disposition::Owned);
    ledger.record(
        ResourceRef::server("srv-1", "fr-par-1"),
        Disposition::Retained,
    );
    let entries = ledger.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries.first().map(|entry| entry.disposition),
        Some(Disposition::Retained)
    );
}

#[rstest]
fn promote_requires_a_recorded_resource(scope: AllocationScope) {
    let ledger = ResourceLedger::in_memory(scope);
    let err = ledger
        .promote(ResourceKind::Server, "missing", Disposition::Retained)
        .expect_err("promotion of an unrecorded resource must fail");
    assert_eq!(
        err,
        LedgerError::NotRecorded {
            kind: ResourceKind::Server,
            id: String::from("missing"),
        }
    );
}

#[rstest]
fn record_persists_every_snapshot(scope: AllocationScope) {
    let sink = Arc::new(RecordingSink::default());
    let ledger = ResourceLedger::with_sink(scope, sink.clone());
    ledger
        .persist_initial()
        .expect("the initial snapshot must persist");
    ledger.record(
        ResourceRef::public_ip("ip-1", "fr-par-1", true),
        Disposition::Owned,
    );
    let snapshots = sink.snapshots.lock().expect("snapshots");
    assert_eq!(snapshots.len(), 2);
    let initial = snapshots.first().expect("the initial snapshot must exist");
    assert_eq!(initial.entries.len(), 0);
    let recorded = snapshots.get(1).expect("the recorded snapshot must exist");
    assert_eq!(recorded.entries.len(), 1);
    assert_eq!(
        recorded
            .entries
            .first()
            .and_then(|entry| entry.resource.ip_dynamic()),
        Some(true),
        "the dynamic IP flag must survive persistence"
    );
}

#[rstest]
fn a_failed_persist_keeps_the_in_memory_record(scope: AllocationScope) {
    let sink = Arc::new(RecordingSink::default());
    let ledger = ResourceLedger::with_sink(scope, sink.clone());
    *sink.fail.lock().expect("fail flag") = true;
    ledger.record(
        ResourceRef::instance_volume("vol-1", "fr-par-1"),
        Disposition::Owned,
    );
    assert_eq!(ledger.entries().len(), 1);
    assert_eq!(ledger.warnings(), vec![String::from("disk full")]);
}

#[rstest]
fn entries_with_filters_by_disposition(scope: AllocationScope) {
    let ledger = ResourceLedger::in_memory(scope);
    ledger.record(ResourceRef::server("srv-1", "fr-par-1"), Disposition::Owned);
    ledger.record(
        ResourceRef::instance_volume("vol-cache", "fr-par-1"),
        Disposition::Retained,
    );
    assert_eq!(
        ledger
            .entries_with(Disposition::Retained)
            .first()
            .map(|entry| entry.resource.id()),
        Some("vol-cache")
    );
    assert_eq!(
        ledger
            .entries_with(Disposition::Owned)
            .first()
            .map(|entry| entry.resource.id()),
        Some("srv-1")
    );
    assert_eq!(
        ledger
            .entries_of(ResourceKind::Server)
            .first()
            .map(|entry| entry.resource.id()),
        Some("srv-1")
    );
}

#[rstest]
fn a_promoted_volume_is_no_longer_deletable(scope: AllocationScope) {
    let ledger = ResourceLedger::in_memory(scope);
    ledger.record(
        ResourceRef::instance_volume("vol-1", "fr-par-1"),
        Disposition::Owned,
    );
    ledger
        .promote(ResourceKind::InstanceVolume, "vol-1", Disposition::Retained)
        .expect("a recorded volume can be promoted");
    let entry = ledger
        .find(ResourceKind::InstanceVolume, "vol-1")
        .expect("the volume stays recorded");
    assert!(!entry.is_deletable());
}
