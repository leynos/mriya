//! Unit tests for journal resolution, atomic writes, and recovery loading.

use camino::Utf8PathBuf;
use rstest::{fixture, rstest};
use tempfile::TempDir;

use super::{JournalError, JournalStore, STATE_DIR_ENV};

mod journal;
mod owners;
use crate::{ownership::AllocationScope, test_helpers::mock_env};

/// Writes `contents` to `path` through a capability on its parent directory.
pub(super) fn write_file(path: &camino::Utf8Path, contents: &str) {
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
pub(super) fn temporary_entries(root: &camino::Utf8Path) -> Result<Vec<String>, String> {
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
pub(super) fn write_staged_file(root: &camino::Utf8Path) {
    write_file(&root.join(".staged.tmp"), "{}");
}

/// Reads `path` through a capability on its parent directory.
pub(super) fn read_text(path: &camino::Utf8Path) -> Result<String, String> {
    use cap_std::ambient_authority;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{path} has no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{path} has no file name"))?;
    let dir = cap_std::fs_utf8::Dir::open_ambient_dir(parent, ambient_authority())
        .map_err(|error| format!("the journal directory must open: {error}"))?;
    dir.read_to_string(name)
        .map_err(|error| format!("the journal must be readable: {error}"))
}

/// Owns a temporary journal directory for one test.
pub(super) struct JournalDir {
    /// Keeps the directory alive for the test's lifetime.
    _temp: TempDir,
    /// The journal store rooted inside the temporary directory.
    pub(super) store: JournalStore,
}

// A fallible fixture would force every consumer to return `Result`, and this
// workspace denies `panic_in_result_fn`, so the assertions that give the tests
// their value could not stay. The failure message still names the operation.
#[fixture]
pub(super) fn journal_dir() -> JournalDir {
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
pub(super) fn scope() -> AllocationScope {
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
