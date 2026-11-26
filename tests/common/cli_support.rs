//! Assertions and step-parameter types shared by the CLI behavioural suites
//! that drive the compiled binary.
//!
//! The isolated invocation itself lives in [`crate::isolated_cli`], which the
//! regression suite shares; this module holds only what the behaviour suites
//! need in common. Assertions return the failure message as a `String` so each
//! suite can wrap it in its own `StepError`.

use crate::isolated_cli::CliOutput;

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

fn captured(output: Option<&CliOutput>) -> Result<&CliOutput, String> {
    output.ok_or_else(|| String::from("missing command output"))
}

/// Checks that the run exited non-zero with `snippet` on stderr.
///
/// # Errors
///
/// Returns the failure message when no output was captured, the run succeeded,
/// or stderr lacks the snippet. Both streams are included, because a run can
/// fail before writing to stderr and the stdout then holds the only clue.
pub fn check_run_failed_with(output: Option<&CliOutput>, snippet: &str) -> Result<(), String> {
    let captured = captured(output)?;
    if captured.status_code == 0 {
        return Err(format!(
            "expected non-zero exit status, got success; stdout: {}; stderr: {}",
            captured.stdout, captured.stderr
        ));
    }
    if captured.stderr.contains(snippet) {
        Ok(())
    } else {
        Err(format!(
            "expected stderr to contain '{snippet}', got: {}; stdout: {}",
            captured.stderr, captured.stdout
        ))
    }
}
