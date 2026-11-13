//! Cases for the build-standard sandbox's `PATH` boundary: what an unset,
//! undecodable, empty, or partial `PATH` reports, and that a usable directory is
//! still searched. They inject the environment, so no case mutates the process.
//!
//! One case cannot inject: the injected boundary takes its answers as
//! `String`, so a genuinely non-UTF-8 `PATH` cannot be expressed through it.
//! That case re-executes this test binary with the value in its real
//! environment, which is the only way the whole-path rejection can be proved
//! against a path that decodes in part.

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

#[cfg(unix)]
mod undecodable_path {
    //! Proves a genuinely non-UTF-8 `PATH` is rejected whole, not decoded in
    //! part. The injected boundary answers with `String`, so this case cannot
    //! use it: it re-executes the test binary with the value in the real
    //! environment, which is the only way to reach the case at all.
    //!
    //! The value is assembled from bytes, which this workspace does only on
    //! Unix. Windows can also hold an unpaired surrogate through
    //! `OsString::from_wide`, but nothing here builds one.

    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    use mockable::DefaultEnv;

    use super::super::sandbox::{Sandbox, find_on_path_with};

    /// The environment variable that makes this binary act as its own child.
    const CHILD_PATH_VAR: &str = "MRIYA_SANDBOX_CHILD_UNDECODABLE_PATH";

    /// The child half of the case below, selected by `--exact`.
    ///
    /// A separate test rather than a flag on the parent: selecting a module
    /// prefix would match nothing and libtest would report success without
    /// running an assertion, which is exactly the failure a probe like this
    /// has to avoid. Returning early keeps it inert under the ordinary run.
    #[test]
    fn the_undecodable_path_report_child() {
        if std::env::var_os(CHILD_PATH_VAR).is_none() {
            return;
        }
        match find_on_path_with("bash", &DefaultEnv) {
            Ok(found) => panic!(
                "the child ran with a decodable PATH and found {found}, so it did not reproduce the case"
            ),
            Err(report) => assert_eq!(report, "PATH is set to a value that is not valid UTF-8"),
        }
    }

    /// Runs the child probe, requiring that it actually ran and passed.
    ///
    /// # Errors
    ///
    /// Returns the reason when the child cannot be spawned, when it reports a
    /// failure, or when it runs no test at all.
    fn run_child(value: &OsString) -> Result<(), String> {
        let exe = std::env::current_exe()
            .map_err(|error| format!("locating the test binary: {error}"))?;
        let output = std::process::Command::new(exe)
            .args([
                "--exact",
                "sandbox_cases::undecodable_path::the_undecodable_path_report_child",
                "--nocapture",
            ])
            .env(CHILD_PATH_VAR, "1")
            .env("PATH", value)
            .output()
            .map_err(|error| format!("running the child probe: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            return Err(format!(
                "the child probe failed ({}): {}",
                output.status,
                stdout.trim()
            ));
        }
        if !stdout.contains("1 passed") {
            return Err(format!(
                "the child probe ran no test, so it proved nothing: {}",
                stdout.trim()
            ));
        }
        Ok(())
    }

    /// A `PATH` of `<valid bin>:/opt/\xff` decodes as far as its first entry,
    /// and that entry holds a `bash` wrapper. If the boundary decoded what it
    /// could, it would find the wrapper and report success; the assertion is
    /// therefore that the undecodable report wins even though a usable
    /// directory is present.
    #[test]
    fn a_non_utf8_path_is_rejected_even_when_a_later_entry_is_usable() {
        if std::env::var_os(CHILD_PATH_VAR).is_some() {
            // The child runs the dedicated child test; it must not spawn one.
            return;
        }

        let sandbox = Sandbox::new().expect("sandbox should be created");
        sandbox
            .write_tool("bash", "exit 0")
            .expect("the usable entry should be written");
        let mut undecodable = sandbox.root().join("bin").into_os_string();
        undecodable.push(OsString::from(":/opt/"));
        undecodable.push(OsString::from_vec(b"\xff".to_vec()));

        run_child(&undecodable).expect("the child should confirm the report");
    }
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
