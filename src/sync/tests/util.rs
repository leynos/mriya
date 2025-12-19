//! Tests for sync module utility helpers.

use super::super::*;

#[test]
fn expand_tilde_expands_home_prefix() {
    let home = std::env::var("HOME").expect("HOME should be set");
    let expanded = expand_tilde("~/.ssh/id_ed25519");
    assert_eq!(expanded, format!("{home}/.ssh/id_ed25519"));
}

#[test]
fn expand_tilde_leaves_absolute_paths_unchanged() {
    let path = "/absolute/path/to/key";
    assert_eq!(expand_tilde(path), path);
}

#[test]
fn expand_tilde_leaves_relative_paths_unchanged() {
    let path = "relative/path/to/key";
    assert_eq!(expand_tilde(path), path);
}
