//! Tests for how SSH option building handles an unusable `HOME`.
//!
//! Every case injects a `MockEnv`, so no test mutates the process
//! environment. The undecodable-`HOME` cases are exactly the ones that would
//! otherwise need an unsafe environment swap.

use std::ffi::OsString;

use rstest::rstest;

use super::super::*;
use super::fixtures::{base_config, networking};
use crate::test_helpers::{mock_env, mock_env_with_raw};

/// Builds a syncer whose identity file is `identity`.
fn syncer_with_identity(identity: &str) -> Syncer<crate::test_support::ScriptedRunner> {
    let cfg = SyncConfig {
        ssh_identity_file: Some(String::from(identity)),
        ..base_config()
    };
    Syncer::new(cfg, crate::test_support::ScriptedRunner::new()).expect("config should validate")
}

#[rstest]
#[case::absolute("/keys/id_ed25519", "/keys/id_ed25519")]
#[case::expanded("~/.ssh/id_ed25519", "/home/tester/.ssh/id_ed25519")]
fn ssh_options_forward_the_expanded_identity_file(#[case] identity: &str, #[case] expected: &str) {
    let syncer = syncer_with_identity(identity);
    let env = mock_env(&[("HOME", "/home/tester")]);

    let args = syncer
        .common_ssh_options(22, &env)
        .expect("the injected HOME is decodable");

    assert!(
        args.contains(&OsString::from(expected)),
        "expected {expected} in {args:?}"
    );
}

#[test]
fn ssh_options_forward_an_unexpanded_tilde_when_home_is_absent() {
    // `ssh` resolves `~` itself when HOME is unset, so passing the path
    // through unchanged is the intended fallback, not a silent failure.
    let syncer = syncer_with_identity("~/.ssh/id_ed25519");
    let env = mock_env(&[]);

    let args = syncer
        .common_ssh_options(22, &env)
        .expect("an absent HOME is not an error");

    assert!(args.contains(&OsString::from("~/.ssh/id_ed25519")));
}

#[test]
fn ssh_options_reject_an_undecodable_home_for_a_tilde_path() {
    let syncer = syncer_with_identity("~/keys/id");
    let env = mock_env_with_raw(|_| {
        Err(std::env::VarError::NotUnicode(OsString::from(
            "/home/\u{fffd}",
        )))
    });

    let result = syncer.common_ssh_options(22, &env);

    assert!(
        matches!(result, Err(SyncError::HomeUnavailable(_))),
        "a tilde path with an undecodable HOME should fail: {result:?}"
    );
}

#[test]
fn ssh_options_ignore_an_undecodable_home_for_an_absolute_path() {
    let syncer = syncer_with_identity("/keys/id");
    let env = mock_env_with_raw(|_| {
        Err(std::env::VarError::NotUnicode(OsString::from(
            "/home/\u{fffd}",
        )))
    });

    let args = syncer
        .common_ssh_options(22, &env)
        .expect("an absolute path needs no HOME");

    assert!(args.contains(&OsString::from("/keys/id")));
}

#[test]
fn ssh_args_propagate_the_home_failure() {
    let syncer = syncer_with_identity("~/keys/id");
    let env = mock_env_with_raw(|_| {
        Err(std::env::VarError::NotUnicode(OsString::from(
            "/home/\u{fffd}",
        )))
    });

    let result = syncer.build_ssh_args(&networking(), "echo ok", &env);

    assert!(
        matches!(result, Err(SyncError::HomeUnavailable(_))),
        "an unusable HOME should reach the SSH argument builder: {result:?}"
    );
}

#[test]
fn rsync_remote_shell_propagates_the_home_failure() {
    let syncer = syncer_with_identity("~/keys/id");
    let env = mock_env_with_raw(|_| {
        Err(std::env::VarError::NotUnicode(OsString::from(
            "/home/\u{fffd}",
        )))
    });

    let result = syncer.build_remote_shell(22, &env);

    assert!(
        matches!(result, Err(SyncError::HomeUnavailable(_))),
        "an unusable HOME should reach the rsync remote shell: {result:?}"
    );
}
