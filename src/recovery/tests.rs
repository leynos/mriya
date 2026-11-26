//! Unit tests for journal resolution, atomic writes, and recovery loading.

use std::sync::Arc;

use camino::Utf8PathBuf;
use rstest::{fixture, rstest};
use tempfile::TempDir;

use super::{JournalError, JournalStore, STATE_DIR_ENV};
use crate::{
    ownership::{
        AllocationScope,
        AllocationTag,
        Disposition,
        LedgerSink,
        ResourceKind,
        ResourceRef,
    },
    test_helpers::mock_env,
};

/// Creates `root` and any missing parents.
fn create_directory(root: &camino::Utf8Path) {
    cap_std::fs_utf8::Dir::create_ambient_dir_all(root, cap_std::ambient_authority())
        .expect("the journal directory must be created");
}

/// Writes `contents` to `path` through a capability on its parent directory.
fn write_file(path: &camino::Utf8Path, contents: &str) {
    use cap_std::ambient_authority;
    let parent = path.parent().expect("the path has a parent");
    let name = path.file_name().expect("the path has a file name");
    let dir = cap_std::fs_utf8::Dir::open_ambient_dir(parent, ambient_authority())
        .expect("the parent directory must open");
    dir.write(name, contents)
        .expect("the file must be writable");
}

/// Returns the names in `root` whose extension is `tmp`, case-insensitively.
///
/// A directory that cannot be inspected is an error, not an empty list: the
/// caller uses the result as proof that no staging file remains.
fn temporary_entries(root: &camino::Utf8Path) -> Result<Vec<String>, String> {
    use cap_std::{ambient_authority, fs_utf8::Dir};
    let dir = Dir::open_ambient_dir(root, ambient_authority())
        .map_err(|error| format!("the journal directory must open: {error}"))?;
    let mut names = Vec::new();
    for entry in dir
        .read_dir(".")
        .map_err(|error| format!("the journal directory must be readable: {error}"))?
    {
        let dir_entry = entry.map_err(|error| format!("an entry must be readable: {error}"))?;
        let name = dir_entry
            .file_name()
            .map_err(|error| format!("a name must be valid UTF-8: {error}"))?;
        if camino::Utf8Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
        {
            names.push(name);
        }
    }
    Ok(names)
}

/// Writes a staged temporary file, which a journal listing must ignore.
fn write_staged_file(root: &camino::Utf8Path) { write_file(&root.join(".staged.tmp"), "{}"); }

/// Owns a temporary journal directory for one test.
struct JournalDir {
    /// Keeps the directory alive for the test's lifetime.
    _temp: TempDir,
    /// The journal store rooted inside the temporary directory.
    store: JournalStore,
}

#[fixture]
fn journal_dir() -> JournalDir {
    let temp = TempDir::new().expect("a temporary directory must be created");
    let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf())
        .expect("the temporary path must be UTF-8")
        .join("mriya")
        .join("runs");
    JournalDir {
        store: JournalStore::at_root(root),
        _temp: temp,
    }
}

#[fixture]
fn scope() -> AllocationScope {
    AllocationScope {
        zone: String::from("fr-par-1"),
        project_id: String::from("project"),
    }
}

/// One environment table and the journal root it must select.
#[derive(Clone, Copy)]
struct ScopeCase {
    /// Variable/value pairs supplied to the loader.
    vars: &'static [(&'static str, &'static str)],
    /// Expected journal root.
    expected: &'static str,
}

#[rstest]
#[case::override_wins(ScopeCase {
    vars: &[
        (STATE_DIR_ENV, "/var/lib/mriya"),
        ("XDG_STATE_HOME", "/xdg"),
        ("HOME", "/home/ada"),
    ],
    expected: "/var/lib/mriya/mriya/runs",
})]
#[case::xdg_when_override_blank(ScopeCase {
    vars: &[
        (STATE_DIR_ENV, "   "),
        ("XDG_STATE_HOME", "/xdg"),
        ("HOME", "/home/ada"),
    ],
    expected: "/xdg/mriya/runs",
})]
#[case::home_default(ScopeCase {
    vars: &[("HOME", "/home/ada")],
    expected: "/home/ada/.local/state/mriya/runs",
})]
#[case::trailing_space_preserved(ScopeCase {
    vars: &[(STATE_DIR_ENV, "/var/lib/mriya ")],
    expected: "/var/lib/mriya /mriya/runs",
})]
fn the_state_directory_is_resolved_in_precedence_order(#[case] case: ScopeCase) {
    let env = mock_env(case.vars);
    let store = JournalStore::from_env(&env).expect("the environment must resolve");
    assert_eq!(store.root(), Utf8PathBuf::from(case.expected));
}

#[rstest]
fn a_missing_state_directory_is_reported() {
    let env = mock_env(&[]);
    let err = JournalStore::from_env(&env).expect_err("no state directory must fail");
    assert!(matches!(err, JournalError::NoStateDir));
}

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
fn an_unsupported_version_is_rejected(journal_dir: JournalDir) {
    let path = journal_dir.store.root().join("future.json");
    journal_dir
        .store
        .journals()
        .expect("the directory may be missing");
    create_directory(journal_dir.store.root());
    write_file(
        &path,
        r#"{"version":99,"allocation":"mriya-alloc-x","scope":{"zone":"z","project_id":"p"},"entries":[]}"#,
    );
    let err = journal_dir
        .store
        .load(&path)
        .expect_err("version 99 must fail");
    assert!(matches!(err, JournalError::Parse { .. }), "{err:?}");
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
