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
    fn command(&self) -> assert_cmd::Command {
        assert_cmd::Command::new(&self.executable)
    }
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
    let mut command = Command::new(env!("CARGO"));
    build_binary_with_command(&mut command)
}

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
        .args([
            "build",
            "--bin",
            "mriya",
            "--features",
            "test-backdoors",
            "--message-format=json-render-diagnostics",
        ])
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
mod tests {
    //! Verifies Cargo output handling and the selected test binary.

    use std::{process::Command, sync::mpsc, thread, time::Duration};

    use super::{BuildError, build_binary_with_command, executable_from_cargo_messages};

    /// Creates one valid JSON artefact line using Cargo's exact protocol reason.
    fn artefact_message(name: &str, kind: &[&str], executable: &str) -> String {
        serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": name, "kind": kind },
            "executable": executable
        })
        .to_string()
    }

    /// Starts a shell script for process-boundary tests without invoking Cargo.
    fn shell_command(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn build_drains_large_stderr_before_selecting_the_binary_artefact() {
        let artefact = artefact_message("mriya", &["bin"], "/not-built/mriya");
        let script =
            format!("head -c 131072 /dev/zero | tr '\\000' x >&2\nprintf '%s\\n' '{artefact}'\n");
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut command = shell_command(&script);
            let result = build_binary_with_command(&mut command);
            sender.send(result)
        });

        let result = receiver.recv_timeout(Duration::from_secs(5));
        assert!(
            matches!(
                &result,
                Ok(Ok(binary)) if binary.executable.as_str() == "/not-built/mriya"
            ),
            "Cargo build should drain both pipes and select mriya: {result:?}"
        );
        assert!(
            matches!(worker.join(), Ok(Ok(()))),
            "fake Cargo worker should finish and report its result"
        );
    }

    #[test]
    fn cargo_failure_preserves_status_and_both_output_streams() {
        let mut command =
            shell_command("printf 'stdout marker\\n'; printf 'stderr marker\\n' >&2; exit 23");
        let error = build_binary_with_command(&mut command).expect_err("fake Cargo should fail");

        let BuildError::CargoFailed {
            status,
            stdout,
            stderr,
        } = &error
        else {
            panic!("Cargo failure should retain its status and diagnostics: {error:?}");
        };
        assert_eq!(status.code(), Some(23));
        assert_eq!(stdout, "stdout marker\n");
        assert_eq!(stderr, "stderr marker\n");
        assert!(error.to_string().contains("stdout marker"));
        assert!(error.to_string().contains("stderr marker"));
    }

    #[test]
    fn artefact_selection_ignores_other_targets_and_non_binary_targets() {
        let messages = [
            artefact_message("other", &["bin"], "/not-built/other"),
            artefact_message("mriya", &["example"], "/not-built/example"),
            artefact_message("mriya", &["bin"], "/not-built/mriya"),
        ]
        .join("\n");

        assert!(matches!(
            executable_from_cargo_messages(&messages),
            Ok(executable) if executable.as_str() == "/not-built/mriya"
        ));
    }

    #[test]
    fn artefact_selection_rejects_output_without_a_matching_binary() {
        let output = r#"{"reason":"compiler-message"}"#;

        assert!(matches!(
            executable_from_cargo_messages(output),
            Err(BuildError::MissingExecutable)
        ));
    }

    #[test]
    fn artefact_selection_rejects_malformed_json() {
        assert!(matches!(
            executable_from_cargo_messages("{not-json}"),
            Err(BuildError::InvalidJson(_))
        ));
    }

    #[test]
    fn artefact_selection_rejects_malformed_json_after_a_matching_artefact() {
        let output = format!(
            "{}\n{{not-json}}",
            artefact_message("mriya", &["bin"], "/not-built/mriya")
        );

        assert!(matches!(
            executable_from_cargo_messages(&output),
            Err(BuildError::InvalidJson(_))
        ));
    }

    #[test]
    fn successful_build_rejects_non_utf8_json_output() {
        let mut command = shell_command(r"printf '\377'");

        assert!(matches!(
            build_binary_with_command(&mut command),
            Err(BuildError::InvalidUtf8(_))
        ));
    }

    #[test]
    fn missing_absolute_executable_preserves_the_spawn_error() {
        let temp_directory = tempfile::tempdir().expect("temporary directory should be created");
        let directory = camino::Utf8Path::from_path(temp_directory.path())
            .expect("temporary directory path should be UTF-8");
        let executable = directory.join("missing-cargo");
        assert!(executable.is_absolute());
        assert!(
            !executable.exists(),
            "fresh temporary directory should not contain the executable"
        );

        let mut command = Command::new(&executable);
        let error = build_binary_with_command(&mut command)
            .expect_err("missing executable should fail before the build starts");

        assert!(matches!(
            error,
            BuildError::Spawn(source) if source.kind() == std::io::ErrorKind::NotFound
        ));
    }
}

#[cfg(test)]
#[path = "test_binary_properties.rs"]
mod properties;
