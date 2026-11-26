//! Tests for the driver helpers that turn repository facts into record fields.
//!
//! These run without a provider: they check that a credential cannot be written
//! into a record through the remote URL, that the tree size is read from the
//! listing rather than guessed, and that a command vector is escaped so the
//! remote shell sees the words the record pins.

use super::super::campaign::{redact_remote, shell_escape, tree_size};

#[test]
fn a_remote_url_with_credentials_is_redacted() {
    assert_eq!(
        redact_remote("https://oauth2:ghp_secret@github.com/leynos/mriya.git"),
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

#[test]
fn a_command_vector_is_escaped_argument_by_argument() {
    let command = vec![
        String::from("sh"),
        String::from("-c"),
        String::from("echo 'hello world'"),
    ];
    let rendered = shell_escape(&command);
    assert!(rendered.starts_with("sh -c "), "{rendered}");
    assert!(rendered.contains("hello world"), "{rendered}");
    // The whole script is one argument, so the spaces inside it are quoted.
    assert!(rendered.contains("'echo '"), "{rendered}");
}
