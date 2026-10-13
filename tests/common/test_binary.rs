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

fn build_test_binary() -> Result<TestBinary, BuildError> {
    let mut command = Command::new(env!("CARGO"));
    build_binary_with_command(&mut command)
}

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

    fn artifact_message(name: &str, kind: &[&str], executable: &str) -> String {
        serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": name, "kind": kind },
            "executable": executable
        })
        .to_string()
    }

    fn shell_command(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn build_drains_large_stderr_before_selecting_the_binary_artifact() {
        let artifact = artifact_message("mriya", &["bin"], "/not-built/mriya");
        let script =
            format!("head -c 131072 /dev/zero | tr '\\000' x >&2\nprintf '%s\\n' '{artifact}'\n");
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
    fn artifact_selection_ignores_other_targets_and_non_binary_targets() {
        let messages = [
            artifact_message("other", &["bin"], "/not-built/other"),
            artifact_message("mriya", &["example"], "/not-built/example"),
            artifact_message("mriya", &["bin"], "/not-built/mriya"),
        ]
        .join("\n");

        assert!(matches!(
            executable_from_cargo_messages(&messages),
            Ok(executable) if executable.as_str() == "/not-built/mriya"
        ));
    }

    #[test]
    fn artifact_selection_rejects_output_without_a_matching_binary() {
        let output = r#"{"reason":"compiler-message"}"#;

        assert!(matches!(
            executable_from_cargo_messages(output),
            Err(BuildError::MissingExecutable)
        ));
    }

    #[test]
    fn artifact_selection_rejects_malformed_json() {
        assert!(matches!(
            executable_from_cargo_messages("{not-json}"),
            Err(BuildError::InvalidJson(_))
        ));
    }

    #[test]
    fn artifact_selection_rejects_malformed_json_after_a_matching_artifact() {
        let output = format!(
            "{}\n{{not-json}}",
            artifact_message("mriya", &["bin"], "/not-built/mriya")
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
}
