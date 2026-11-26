//! Journal-owner identity and replay-claim tests.

use std::sync::Arc;

use rstest::rstest;

use super::{JournalDir, journal_dir, scope, write_file};
use crate::ownership::{AllocationScope, AllocationTag, LedgerSink};

#[rstest]
fn a_persisted_journal_records_its_owner(journal_dir: JournalDir, scope: AllocationScope) {
    let tag = AllocationTag::generate();
    let journal = journal_dir.store.handle(&tag);
    let ledger = crate::ownership::ResourceLedger::with_tag(
        scope,
        tag,
        Some(Arc::new(journal.clone()) as Arc<dyn LedgerSink>),
    );
    ledger
        .persist_initial()
        .expect("the initial journal must persist");

    let record = journal_dir
        .store
        .load(journal.path())
        .expect("the journal must load");
    let owner = record.owner.expect("a live run must record its owner");
    assert_eq!(owner.pid, std::process::id());
    assert!(owner.is_alive(), "the current process is alive");
}

#[rstest]
fn a_journal_without_an_owner_is_still_recoverable(
    journal_dir: JournalDir,
    scope: AllocationScope,
) -> Result<(), String> {
    cap_std::fs_utf8::Dir::create_ambient_dir_all(
        journal_dir.store.root(),
        cap_std::ambient_authority(),
    )
    .expect("the journal directory must be created");
    let path = journal_dir.store.root().join("mriya-alloc-ownerless.json");
    write_file(
        &path,
        &format!(
            r#"{{"version":1,"allocation":"mriya-alloc-ownerless","scope":{{"zone":"{zone}","project_id":"{project}"}},"entries":[],"outcome_unknown":false}}"#,
            zone = scope.zone,
            project = scope.project_id
        ),
    );
    let record = journal_dir
        .store
        .load(&path)
        .map_err(|error| error.to_string())?;
    if record.owner.is_some() {
        return Err(String::from("an ownerless journal must keep no owner"));
    }
    Ok(())
}

#[rstest]
fn a_dead_owner_is_not_reported_alive(journal_dir: JournalDir, scope: AllocationScope) {
    cap_std::fs_utf8::Dir::create_ambient_dir_all(
        journal_dir.store.root(),
        cap_std::ambient_authority(),
    )
    .expect("the journal directory must be created");
    let tag = AllocationTag::generate();
    let journal = journal_dir.store.handle(&tag);
    let mut record = crate::recovery::JournalRecord {
        version: 1,
        allocation: tag,
        scope,
        entries: Vec::new(),
        outcome_unknown: false,
        // A process id the kernel has no start time for cannot be alive.
        owner: Some(crate::recovery::ProcessIdentity {
            pid: u32::MAX,
            start_time: 0,
        }),
    };
    record.version = 1;
    let path = journal.path().to_path_buf();
    write_file(
        &path,
        &serde_json::to_string(&record).expect("the record must serialize"),
    );
    let loaded = journal_dir
        .store
        .load(&path)
        .expect("the journal must load");
    let owner = loaded.owner.expect("the owner must round-trip");
    assert!(!owner.is_alive(), "a stale owner must not count as alive");
}
