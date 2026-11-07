//! Tests for sync module utility helpers.

use super::super::*;
use crate::test_helpers::mock_env;

#[test]
fn expand_tilde_with_env_expands_home_prefix() {
    let env = mock_env(&[("HOME", "/home/tester")]);
    assert_eq!(
        expand_tilde_with_env("~/.ssh/id_ed25519", &env),
        "/home/tester/.ssh/id_ed25519"
    );
}

#[test]
fn expand_tilde_with_env_leaves_path_unchanged_without_home() {
    let env = mock_env(&[]);
    assert_eq!(
        expand_tilde_with_env("~/.ssh/id_ed25519", &env),
        "~/.ssh/id_ed25519"
    );
}

#[test]
fn expand_tilde_reads_process_home() {
    let expanded = temp_env::with_var("HOME", Some("/home/scoped"), || {
        expand_tilde("~/.ssh/id_ed25519")
    });
    assert_eq!(expanded, "/home/scoped/.ssh/id_ed25519");
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
