//! Persistence, loading, listing, and removal of recovery journals.

use std::sync::Arc;

use rstest::rstest;

use super::{
    JournalDir,
    journal_dir,
    read_text,
    scope,
    temporary_entries,
    write_file,
    write_staged_file,
};
use crate::{
    ownership::{
        AllocationScope,
        AllocationTag,
        Disposition,
        LedgerSink,
        ResourceKind,
        ResourceRef,
    },
    recovery::JournalError,
};

#[rstest]
fn journal_path_embeds_the_allocation_tag(journal_dir: JournalDir) {
    let tag = AllocationTag::parse("mriya-alloc-abc123").expect("the tag must parse");
    assert_eq!(
        journal_dir.store.journal_path(&tag),
        journal_dir.store.root().join("mriya-alloc-abc123.json")
    );
}

#[rstest]
fn persist_then_load_round_trips_and_restricts_permissions(
    journal_dir: JournalDir,
    scope: AllocationScope,
) {
    let tag = AllocationTag::generate();
    let journal = journal_dir.store.handle(&tag);
    let ledger = crate::ownership::ResourceLedger::with_tag(
        scope,
        tag.clone(),
        Some(Arc::new(journal.clone()) as Arc<dyn LedgerSink>),
    );
    ledger
        .persist_initial()
        .expect("the initial journal must persist");
    ledger.record(ResourceRef::server("srv-1", "fr-par-1"), Disposition::Owned);

    let record = journal_dir
        .store
        .load(journal.path())
        .expect("the journal must load");
    assert_eq!(record.allocation, tag);
    assert_eq!(record.entries.len(), 1);

    #[cfg(unix)]
    {
        use cap_std::{
            ambient_authority,
            fs_utf8::{Dir, PermissionsExt as _},
        };
        let parent = journal
            .path()
            .parent()
            .expect("the journal path has a parent");
        let dir = Dir::open_ambient_dir(parent, ambient_authority())
            .expect("the journal directory must open");
        let mode = dir
            .metadata(journal.path().file_name().expect("the journal has a name"))
            .expect("the journal must exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "journals must not be group- or world-readable");
    }

    let leftovers =
        temporary_entries(journal_dir.store.root()).expect("the staging check must run");
    assert_eq!(
        leftovers.len(),
        0,
        "an atomic write must leave no temporary file: {leftovers:?}"
    );
}

#[rstest]
fn a_second_persist_replaces_the_first(journal_dir: JournalDir, scope: AllocationScope) {
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
    ledger.record(
        ResourceRef::instance_volume("vol-1", "fr-par-1"),
        Disposition::Retained,
    );

    let record = journal_dir
        .store
        .load(journal.path())
        .expect("the journal must load");
    assert_eq!(record.entries.len(), 1);
    assert_eq!(
        record.entries.first().map(|entry| entry.disposition),
        Some(Disposition::Retained)
    );
}

#[rstest]
fn journals_lists_only_complete_journals(journal_dir: JournalDir, scope: AllocationScope) {
    let first = journal_dir.store.handle(&AllocationTag::generate());
    let second = journal_dir.store.handle(&AllocationTag::generate());
    for journal in [&first, &second] {
        let ledger = crate::ownership::ResourceLedger::with_tag(
            scope.clone(),
            AllocationTag::parse(
                journal
                    .path()
                    .file_stem()
                    .expect("the journal path has a stem"),
            )
            .expect("the stem is a valid tag"),
            Some(Arc::new(journal.clone()) as Arc<dyn LedgerSink>),
        );
        ledger
            .persist_initial()
            .expect("the initial journal must persist");
    }
    write_staged_file(journal_dir.store.root());

    let listed = journal_dir.store.journals().expect("listing must succeed");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|path| path == first.path()));
    assert!(listed.iter().any(|path| path == second.path()));
}

#[rstest]
fn a_missing_directory_lists_no_journals(journal_dir: JournalDir) {
    assert_eq!(
        journal_dir
            .store
            .journals()
            .expect("a missing directory is not an error")
            .len(),
        0
    );
}

#[rstest]
fn removing_a_journal_is_idempotent(journal_dir: JournalDir, scope: AllocationScope) {
    let journal = journal_dir.store.handle(&AllocationTag::generate());
    let ledger = crate::ownership::ResourceLedger::with_tag(
        scope,
        AllocationTag::generate(),
        Some(Arc::new(journal.clone()) as Arc<dyn LedgerSink>),
    );
    ledger
        .persist_initial()
        .expect("the initial journal must persist");
    assert!(
        journal.path().exists(),
        "the journal must exist before the first removal"
    );
    journal.remove().expect("the first removal must succeed");
    assert!(
        !journal.path().exists(),
        "the first removal must delete the journal"
    );
    journal
        .remove()
        .expect("removing an absent journal must still succeed");
}

#[rstest]
fn an_unsupported_version_is_rejected(journal_dir: JournalDir) -> Result<(), String> {
    // Write a record the loader accepts, then change only its version, so a
    // failure here proves the version check ran rather than the parser.
    let tag = AllocationTag::generate();
    let journal = journal_dir.store.handle(&tag);
    let ledger = crate::ownership::ResourceLedger::with_tag(
        AllocationScope {
            zone: String::from("fr-par-1"),
            project_id: String::from("project"),
        },
        tag,
        Some(Arc::new(journal.clone()) as Arc<dyn LedgerSink>),
    );
    ledger
        .persist_initial()
        .expect("the journal must be written");
    let contents = read_text(journal.path())?;
    let mut record: serde_json::Value =
        serde_json::from_str(&contents).map_err(|error| error.to_string())?;
    record
        .as_object_mut()
        .ok_or_else(|| String::from("the journal must be a JSON object"))?
        .insert(String::from("version"), serde_json::json!(99));
    write_file(
        journal.path(),
        &serde_json::to_string(&record).map_err(|error| error.to_string())?,
    );
    let err = journal_dir
        .store
        .load(journal.path())
        .expect_err("version 99 must fail");
    let JournalError::Parse { message, .. } = err else {
        return Err(format!("expected a parse failure, got {err:?}"));
    };
    if !message.contains("unsupported journal version 99") {
        return Err(format!("the version must be named, got: {message}"));
    }
    Ok(())
}

#[rstest]
fn a_claim_excludes_a_second_recovery(journal_dir: JournalDir, scope: AllocationScope) {
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

    let first = journal_dir
        .store
        .claim(journal.path())
        .expect("claiming a free journal must succeed");
    assert!(first.is_some(), "the first claim must be granted");
    assert!(
        journal_dir
            .store
            .claim(journal.path())
            .expect("the contention check must run")
            .is_none(),
        "a held claim must exclude a second recovery"
    );
    drop(first);
    assert!(
        journal_dir
            .store
            .claim(journal.path())
            .expect("the released claim must be available")
            .is_some(),
        "dropping the claim must release it"
    );
}

#[rstest]
fn a_recovered_ledger_preloads_and_re_persists_the_record(
    journal_dir: JournalDir,
    scope: AllocationScope,
) {
    let tag = AllocationTag::generate();
    let journal = journal_dir.store.handle(&tag);
    let written = crate::ownership::ResourceLedger::with_tag(
        scope,
        tag,
        Some(Arc::new(journal.clone()) as Arc<dyn LedgerSink>),
    );
    written.record(
        ResourceRef::block_volume("vol-block", "fr-par-1"),
        Disposition::Owned,
    );

    let (record, sink) = journal_dir
        .store
        .open(journal.path())
        .expect("the journal must open");
    let recovered = record.into_ledger(sink);
    assert_eq!(
        recovered
            .find(ResourceKind::BlockVolume, "vol-block")
            .expect("the entry must be preloaded")
            .disposition,
        Disposition::Owned
    );

    recovered
        .promote(
            ResourceKind::BlockVolume,
            "vol-block",
            Disposition::Retained,
        )
        .expect("the recovered entry can be promoted");
    let reloaded = journal_dir
        .store
        .load(journal.path())
        .expect("the rewritten journal must load");
    assert_eq!(
        reloaded.entries.first().map(|entry| entry.disposition),
        Some(Disposition::Retained)
    );
}
