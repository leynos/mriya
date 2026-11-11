//! Cases for the build-standard sandbox's `PATH` boundary: what an unset,
//! undecodable, empty, or partial `PATH` reports, and that a usable directory is
//! still searched. They inject the environment, so no case mutates the process.

use std::ffi::OsString;

use mockable::MockEnv;

use super::sandbox::{Sandbox, find_on_path_with, is_tool};

/// Builds an environment whose `PATH` and nothing else is answered by `responds`.
fn env_reporting_path(
    responds: impl Fn() -> Result<String, std::env::VarError> + Send + Sync + 'static,
) -> MockEnv {
    let shared = std::sync::Arc::new(responds);
    let mut env = MockEnv::new();
    let for_raw = std::sync::Arc::clone(&shared);
    env.expect_raw().returning(move |_| for_raw());
    env.expect_string().returning(move |_| shared().ok());
    env
}

#[test]
fn unset_path_is_reported_as_unset() {
    let env = env_reporting_path(|| Err(std::env::VarError::NotPresent));

    let err = find_on_path_with("bash", &env).expect_err("an unset PATH cannot be searched");

    assert_eq!(err, "PATH is not set");
}

#[test]
fn undecodable_path_is_reported_as_undecodable() {
    // The distinction matters: the environment is present but unusable, so
    // the reader must not go looking for a variable that is already set.
    let env = env_reporting_path(|| {
        Err(std::env::VarError::NotUnicode(OsString::from(
            "/opt/\u{fffd}",
        )))
    });

    let err = find_on_path_with("bash", &env).expect_err("an undecodable PATH cannot be searched");

    assert_eq!(err, "PATH is set to a value that is not valid UTF-8");
}

#[test]
fn a_partial_path_still_finds_a_program_in_a_later_directory() {
    let sandbox = Sandbox::new().expect("sandbox should be created");
    sandbox
        .write_tool("probe", "exit 0")
        .expect("probe should be written");
    let bin = sandbox.root().join("bin");
    let for_env = bin.clone();
    let env = env_reporting_path(move || Ok(format!("/nonexistent:{for_env}")));

    let found = find_on_path_with("probe", &env).expect("the second entry holds the tool");

    assert_eq!(found, bin.join("probe"));
}

#[test]
fn an_empty_path_reports_the_program_as_missing() {
    let env = env_reporting_path(|| Ok(String::new()));

    let err = find_on_path_with("bash", &env).expect_err("an empty PATH cannot be searched");

    assert!(
        err.contains("not on PATH"),
        "an empty PATH should read as no directories: {err}"
    );
}

#[test]
fn is_tool_reports_only_a_readable_file() {
    let sandbox = Sandbox::new().expect("sandbox should be created");
    sandbox
        .write_tool("probe", "exit 0")
        .expect("probe should be written");
    let bin = sandbox.root().join("bin");

    assert!(is_tool(&bin, "probe").expect("the tool directory should be readable"));
    assert!(!is_tool(&bin, "absent").expect("the tool directory should be readable"));
}
