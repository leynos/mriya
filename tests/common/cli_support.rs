//! Support shared by the CLI behavioural suites that drive the compiled binary
//! in fake request-dump mode: captured process output, the base command, the
//! string newtypes used for step parameters, and the field and failure
//! assertions over that output.
//!
//! Assertions return the failure message as a `String` so each suite can wrap
//! it in its own `StepError`.

use std::process::Output;

pub const DEFAULT_IMAGE_LABEL: &str = "Ubuntu 24.04 Noble Numbat";
const DUMMY_SECRET_KEY: &str = "dummy-secret";
const DUMMY_PROJECT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// Declares a `String`-backed newtype usable as a BDD step parameter.
macro_rules! string_newtype {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        struct $name(String);

        impl From<String> for $name {
            fn from(value: String) -> Self { Self(value) }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str { self.0.as_str() }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_ref())
            }
        }

        impl std::str::FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(value: &str) -> Result<Self, Self::Err> { Ok(Self(value.to_owned())) }
        }
    };
}
pub(crate) use string_newtype;

/// Captured result of one CLI invocation.
#[derive(Clone, Debug)]
pub struct CliOutput {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    pub fn from_process_output(output: Output) -> Self {
        let Output {
            status,
            stdout: raw_stdout,
            stderr: raw_stderr,
        } = output;
        Self {
            status_code: status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&raw_stdout).into_owned(),
            stderr: String::from_utf8_lossy(&raw_stderr).into_owned(),
        }
    }
}

/// Creates a command for the lazily compiled, feature-enabled test binary.
///
/// The command selects fake request-dump mode and applies the given instance
/// type and image defaults plus dummy Scaleway credentials. Other environment
/// variables remain inherited from the test process. If Cargo exits with a
/// failure, `BuildError` retains its status, stdout, and stderr; launch and
/// artefact-selection failures are also returned as `BuildError`.
///
/// # Example
///
/// Add `run` options to the returned command, then inspect the captured
/// request dump to verify how the options were represented.
pub fn fake_run_command(
    default_instance_type: &str,
    default_image: &str,
) -> Result<assert_cmd::Command, &'static crate::test_binary::BuildError> {
    let mut cmd = crate::test_binary::mriya_cmd()?;
    cmd.env("MRIYA_FAKE_RUN_ENABLE", "1");
    cmd.env("MRIYA_FAKE_RUN_MODE", "dump-request");
    cmd.env("SCW_SECRET_KEY", DUMMY_SECRET_KEY);
    cmd.env("SCW_DEFAULT_PROJECT_ID", DUMMY_PROJECT_ID);
    cmd.env("SCW_DEFAULT_INSTANCE_TYPE", default_instance_type);
    cmd.env("SCW_DEFAULT_IMAGE", default_image);
    Ok(cmd)
}

fn captured(output: Option<&CliOutput>) -> Result<&CliOutput, String> {
    output.ok_or_else(|| String::from("missing command output"))
}

/// Finds the value printed as `key=value` on its own line of `stdout`.
fn extract_value(stdout: &str, key: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .map(str::to_owned)
}

/// Checks that the dumped request reports `expected` for `key`.
///
/// # Errors
///
/// Returns the failure message when no output was captured, the key is
/// absent, or its value differs.
pub fn check_field_value(
    output: Option<&CliOutput>,
    key: &str,
    expected: &str,
) -> Result<(), String> {
    let captured = captured(output)?;
    let actual = extract_value(&captured.stdout, key)
        .ok_or_else(|| format!("stdout missing {key} key, got: {}", captured.stdout))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected {key} '{expected}', got '{actual}'"))
    }
}

/// Checks that the run exited non-zero with `snippet` on stderr.
///
/// # Errors
///
/// Returns the failure message when no output was captured, the run
/// succeeded, or stderr lacks the snippet.
pub fn check_run_failed_with(output: Option<&CliOutput>, snippet: &str) -> Result<(), String> {
    let captured = captured(output)?;
    if captured.status_code == 0 {
        return Err(String::from("expected non-zero exit status"));
    }
    if captured.stderr.contains(snippet) {
        Ok(())
    } else {
        Err(format!(
            "expected stderr to contain '{snippet}', got: {}",
            captured.stderr
        ))
    }
}
