//! Tests for sync module path helpers.

use std::ffi::OsString;

use rstest::rstest;

use super::super::*;
use crate::test_helpers::{mock_env, mock_env_with_raw};

#[rstest]
#[case::home_set(
    "~/.ssh/id_ed25519",
    Some("/home/tester"),
    "/home/tester/.ssh/id_ed25519"
)]
#[case::home_absent("~/.ssh/id_ed25519", None, "~/.ssh/id_ed25519")]
#[case::bare_tilde("~", Some("/home/tester"), "~")]
#[case::trailing_tilde("key~", Some("/home/tester"), "key~")]
#[case::inner_tilde("/opt/~/key", Some("/home/tester"), "/opt/~/key")]
#[case::no_tilde("/absolute/key", Some("/home/tester"), "/absolute/key")]
#[case::relative("relative/key", Some("/home/tester"), "relative/key")]
#[case::rooted_home("~/", Some("/home/tester"), "/home/tester/")]
fn expand_tilde_with_home_expands_only_a_leading_tilde(
    #[case] path: &str,
    #[case] home: Option<&str>,
    #[case] expected: &str,
) {
    assert_eq!(expand_tilde_with_home(path, home), expected);
}

#[test]
fn home_from_env_reads_home_for_a_tilde_path() {
    let env = mock_env_with_raw(|_| Ok(String::from("/home/tester")));

    assert_eq!(
        home_from_env(&env, "~/.ssh/id_ed25519").expect("HOME should be decodable"),
        Some(String::from("/home/tester"))
    );
}

#[test]
fn home_from_env_skips_the_lookup_for_a_path_without_tilde() {
    // A panicking expectation proves the variable is never consulted: a path
    // that needs no home directory must not fail just because HOME is absent.
    let mut env = mockable::MockEnv::new();
    env.expect_raw()
        .returning(|_| panic!("a non-tilde path must not read HOME"));

    assert_eq!(
        home_from_env(&env, "/opt/key").expect("no lookup should happen"),
        None
    );
}

#[test]
fn home_from_env_reports_absent_home_as_no_home() {
    let env = mock_env(&[]);

    assert_eq!(
        home_from_env(&env, "~/.ssh/id_ed25519").expect("absence is not a failure"),
        None
    );
}

#[test]
fn home_from_env_rejects_undecodable_home() {
    let env = mock_env_with_raw(|_| {
        Err(std::env::VarError::NotUnicode(OsString::from(
            "/home/\u{fffd}",
        )))
    });

    let err = home_from_env(&env, "~/.ssh/id_ed25519").expect_err("bad HOME should be reported");

    assert_eq!(err, HomeUnavailable);
}
