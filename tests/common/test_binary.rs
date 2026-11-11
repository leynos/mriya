//! Builds and exposes the feature-enabled binary used by CLI tests.

use std::{
    process::{Command, ExitStatus, Output},
    sync::LazyLock,
};

use camino::Utf8PathBuf;
use serde::Deserialize;

/// Describes failures while building the CLI test binary.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildError {
    /// Cargo could not be started.
    #[error("failed to start Cargo build: {0}")]
    Spawn(#[source] std::io::Error),
    /// Cargo failed, with both output streams retained for diagnosis.
    #[error("Cargo build failed with status {status}\nstdout:\n{stdout}\nstderr:\n{stderr}")]
    CargoFailed {
        /// The status returned by Cargo.
        status: ExitStatus,
        /// Cargo's stdout, decoded as text.
        stdout: String,
        /// Cargo's stderr, decoded as text.
        stderr: String,
    },
    /// Cargo emitted a line that was not a valid JSON build message.
    #[error("invalid Cargo build message: {0}")]
    InvalidJson(#[from] serde_json::Error),
    /// Cargo emitted successful build output that was not valid UTF-8.
    #[error("Cargo build output was not valid UTF-8: {0}")]
    InvalidUtf8(#[source] std::str::Utf8Error),
    /// Cargo succeeded without reporting the requested binary executable.
    #[error("Cargo build did not report an executable for the mriya binary")]
    MissingExecutable,
}

#[derive(Debug)]
struct TestBinary {
    executable: Utf8PathBuf,
}

impl TestBinary {
    /// Creates a fresh command for one CLI assertion sequence.
    ///
    /// Each caller receives its own argument builder while sharing the cached
    /// executable selected for this test process.
    ///
    /// For example, two calls give separate empty argument lists that invoke
    /// the same executable.
    fn command(&self) -> assert_cmd::Command { assert_cmd::Command::new(&self.executable) }
}

static MRIYA_BIN: LazyLock<Result<TestBinary, BuildError>> = LazyLock::new(build_test_binary);

/// Creates a fresh command for the lazily built `mriya` binary.
///
/// # Examples
///
/// ```rust,no_run
/// let mut command = test_binary::mriya_cmd()?;
/// command.arg("--help");
/// # Ok::<(), &'static test_binary::BuildError>(())
/// ```
pub(crate) fn mriya_cmd() -> Result<assert_cmd::Command, &'static BuildError> {
    MRIYA_BIN.as_ref().map(TestBinary::command)
}

/// Builds the test binary using the Cargo executable and environment inherited
/// by the integration-test process.
///
/// Cargo's configured target directory and shared cache remain in effect, so
/// the reported executable is the one Cargo actually produced for this build.
///
/// For example, the first `mriya_cmd()` call initializes the cached binary;
/// later calls reuse its executable path.
fn build_test_binary() -> Result<TestBinary, BuildError> {
    // The dependencies are fetched before the tests start, so this build needs
    // no network and must not wait on the index. The command is built and run
    // in one chain so the offline setting is visible where cargo starts.
    let output = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("CARGO_NET_OFFLINE", "true")
        .args(BUILD_ARGS)
        .output()
        .map_err(BuildError::Spawn)?;

    binary_from_output(&output)
}

/// The arguments of the CLI-test build request.
const BUILD_ARGS: [&str; 6] = [
    "build",
    "--bin",
    "mriya",
    "--features",
    "test-backdoors",
    "--message-format=json-render-diagnostics",
];

/// Adds the CLI-test build request to a command and captures its completed
/// process output.
///
/// `Command::output` drains stdout and stderr together, avoiding a pipe-capacity
/// deadlock when Cargo emits a large diagnostic stream. Spawn and build errors
/// remain distinct for callers and tests.
///
/// For example, a missing program returns `Spawn`, while a process that exits
/// unsuccessfully returns `CargoFailed` with both streams retained.
fn build_binary_with_command(command: &mut Command) -> Result<TestBinary, BuildError> {
    let output = command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        // The dependencies are fetched before the tests start, so this build
        // needs no network and must not wait on the index.
        .env("CARGO_NET_OFFLINE", "true")
        .args(BUILD_ARGS)
        .output()
        .map_err(BuildError::Spawn)?;

    binary_from_output(&output)
}

/// Converts Cargo's completed build output into the executable it reported.
///
/// Failed builds retain both captured streams for diagnosis; successful stdout
/// must be UTF-8 containing Cargo JSON messages.
///
/// For example, a successful output with a matching binary record yields its
/// reported path; a non-zero status yields `CargoFailed` with captured output.
fn binary_from_output(output: &Output) -> Result<TestBinary, BuildError> {
    if !output.status.success() {
        return Err(BuildError::CargoFailed {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let stdout = std::str::from_utf8(&output.stdout).map_err(BuildError::InvalidUtf8)?;
    let executable = executable_from_cargo_messages(stdout)?;
    Ok(TestBinary { executable })
}

/// Selects the last reported executable from matching Cargo artefact records.
///
/// The external reason value is Cargo's exact `"compiler-artifact"` string.
/// Every nonempty line is parsed, so malformed later messages remain errors;
/// matching records without an executable leave the last reported one intact.
///
/// For example, if two matching artefacts provide paths, the later path wins;
/// a later matching record without a path leaves that selection unchanged.
fn executable_from_cargo_messages(stdout: &str) -> Result<Utf8PathBuf, BuildError> {
    let mut executable = None;
    for line in stdout.lines().filter(|line| !line.is_empty()) {
        let message: CargoMessage = serde_json::from_str(line)?;
        if is_mriya_binary(&message) {
            executable = message.executable.map(Utf8PathBuf::from).or(executable);
        }
    }

    executable.ok_or(BuildError::MissingExecutable)
}

/// Recognizes a Cargo `"compiler-artifact"` record for the `mriya` binary target.
///
/// For example, reason `"compiler-artifact"`, target `"mriya"`, and kind
/// `"bin"` match, while an `"example"` kind does not.
fn is_mriya_binary(message: &CargoMessage) -> bool {
    message.reason == "compiler-artifact"
        && message.target.as_ref().is_some_and(|target| {
            target.name == "mriya" && target.kind.iter().any(|kind| kind == "bin")
        })
}

#[derive(Deserialize)]
struct CargoMessage {
    reason: String,
    target: Option<CargoTarget>,
    executable: Option<String>,
}

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
}

#[cfg(test)]
#[path = "test_binary_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "test_binary_properties.rs"]
mod properties;
