//! Tests for the driver helpers that turn repository facts into record fields.
//!
//! These run without a provider: they check that a credential cannot be written
//! into a record through the remote URL, that the tree size is read from the
//! listing rather than guessed, and that a command vector is escaped so the
//! remote shell sees the words the record pins.

use rstest::rstest;

use super::super::campaign::{redact_remote, shell_escape, tree_size};

#[test]
fn a_remote_url_with_credentials_is_redacted() {
    assert_eq!(
        redact_remote("https://oauth2:ghp_secret@github.com/leynos/mriya.git"),
        "https://github.com/leynos/mriya.git"
    );
    // A password may itself contain `@`; splitting at the first one would
    // leave its tail in the record.
    assert_eq!(
        redact_remote("https://user:pa@ss@github.com/leynos/mriya.git"),
        "https://github.com/leynos/mriya.git"
    );
    // An SSH URL's userinfo is a login name, not a credential, so it stays.
    assert_eq!(
        redact_remote("ssh://git@github.com/leynos/mriya.git"),
        "ssh://git@github.com/leynos/mriya.git"
    );
    assert_eq!(
        redact_remote("git@github.com:leynos/mriya.git"),
        "git@github.com:leynos/mriya.git"
    );
}

#[test]
fn a_tree_listing_gives_the_file_count_and_total_size() {
    let listing = concat!(
        "100644 blob aaaa 12\tREADME.md\n",
        "100644 blob bbbb 4096\tsrc/lib.rs\n",
        "040000 tree cccc -\tsrc\n",
    );
    let (count, bytes) = tree_size(listing);
    assert_eq!(count, 3);
    assert_eq!(bytes, 12 + 4096);
}

#[rstest]
#[case(vec!["sh", "-c", "echo 'hello world'"], "sh -c 'echo '\\''hello world'\\'''")]
#[case(vec!["cargo", "build", "a b"], "cargo build 'a b'")]
#[case(vec!["echo", ""], "echo ''")]
#[case(vec!["sh", "-c", "a; rm -rf /#"], "sh -c 'a; rm -rf /#'")]
fn a_command_vector_is_escaped_argument_by_argument(
    #[case] command: Vec<&str>,
    #[case] expected: &str,
) {
    let owned: Vec<String> = command.into_iter().map(str::to_owned).collect();
    assert_eq!(shell_escape(&owned), expected);
}
