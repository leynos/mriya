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

/// Builds an environment whose `HOME` cannot be decoded.
///
/// One shared constructor keeps the three undecodable-`HOME` cases from
/// drifting into subtly different environments.
fn env_with_undecodable_home() -> mockable::MockEnv {
    mock_env_with_raw(|_| {
        Err(std::env::VarError::NotUnicode(OsString::from(
            "/home/\u{fffd}",
        )))
    })
}

/// The three entry points that expand a `~/` identity, so the failure case
/// below can prove each one propagates it rather than swallowing it.
#[rstest]
#[case::common_options(
    |syncer: &Syncer<crate::test_support::ScriptedRunner>, env: &mockable::MockEnv| {
        syncer.common_ssh_options(22, env).map(|_| ())
    }
)]
#[case::ssh_args(
    |syncer: &Syncer<crate::test_support::ScriptedRunner>, env: &mockable::MockEnv| {
        syncer.build_ssh_args(&networking(), "echo ok", env).map(|_| ())
    }
)]
#[case::remote_shell(
    |syncer: &Syncer<crate::test_support::ScriptedRunner>, env: &mockable::MockEnv| {
        syncer.build_remote_shell(22, env).map(|_| ())
    }
)]
fn an_undecodable_home_reaches_every_expansion_site(
    #[case] run: fn(
        &Syncer<crate::test_support::ScriptedRunner>,
        &mockable::MockEnv,
    ) -> Result<(), SyncError>,
) {
    let syncer = syncer_with_identity("~/keys/id");
    let env = env_with_undecodable_home();

    let result = run(&syncer, &env);

    assert!(
        matches!(result, Err(SyncError::HomeUnavailable(_))),
        "an unusable HOME should be reported at this boundary: {result:?}"
    );
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
fn ssh_options_ignore_an_undecodable_home_for_an_absolute_path() {
    let syncer = syncer_with_identity("/keys/id");
    let env = env_with_undecodable_home();

    let args = syncer
        .common_ssh_options(22, &env)
        .expect("an absolute path needs no HOME");

    assert!(args.contains(&OsString::from("/keys/id")));
}
