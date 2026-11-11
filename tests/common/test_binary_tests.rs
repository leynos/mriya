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

/// Emits stderr beyond pipe capacity before a valid artefact record and
/// checks the build helper completes within a bounded wait.
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

/// Checks that a non-zero Cargo status and both diagnostic streams survive
/// conversion to `BuildError::CargoFailed`.
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

/// Checks that unrelated targets and non-binary `mriya` artefacts are
/// ignored when selecting the compiled CLI executable.
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

/// Checks the failure boundary when Cargo succeeds without a matching
/// `mriya` binary record.
#[test]
fn artefact_selection_rejects_output_without_a_matching_binary() {
    let output = r#"{"reason":"compiler-message"}"#;

    assert!(matches!(
        executable_from_cargo_messages(output),
        Err(BuildError::MissingExecutable)
    ));
}

/// Checks that malformed Cargo JSON is reported as `InvalidJson`.
#[test]
fn artefact_selection_rejects_malformed_json() {
    assert!(matches!(
        executable_from_cargo_messages("{not-json}"),
        Err(BuildError::InvalidJson(_))
    ));
}

/// Checks that a malformed later record still fails even after a matching
/// executable record has appeared.
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

/// Checks that non-UTF-8 successful build output fails before JSON
/// artefact selection.
#[test]
fn successful_build_rejects_non_utf8_json_output() {
    let mut command = shell_command(r"printf '\377'");

    assert!(matches!(
        build_binary_with_command(&mut command),
        Err(BuildError::InvalidUtf8(_))
    ));
}

/// Uses a missing absolute executable path to check that process-start
/// failures remain `Spawn(NotFound)` rather than becoming build failures.
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
