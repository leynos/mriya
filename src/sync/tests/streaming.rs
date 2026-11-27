//! Tests for `StreamingCommandRunner` output forwarding and capture.

use std::{ffi::OsString, fmt::Write as _};

use rstest::rstest;

use super::super::*;

/// Run `script` through `StreamingCommandRunner` as `sh -c script`.
///
/// Running the command can fail, so this reports the runner's error rather
/// than raising it: the calling test body unwraps, where a failure is the
/// verdict and carries the test's own line.
fn run_streaming_script(script: &str) -> Result<CommandOutput, SyncError> {
    StreamingCommandRunner.run("sh", &[OsString::from("-c"), OsString::from(script)])
}

#[rstest]
fn streaming_runner_captures_output() {
    let output = run_streaming_script("printf out && printf err 1>&2")
        .expect("command should execute successfully");

    assert_eq!(output.code, Some(0));
    assert_eq!(output.stdout, "out");
    assert_eq!(output.stderr, "err");
}

#[rstest]
fn streaming_runner_captures_output_on_failure() {
    // A non-zero exit status is the runner's report, not a runner error, so
    // the call must succeed and hand the code back untouched.
    let output = run_streaming_script("printf out && printf err 1>&2; exit 42")
        .expect("command should execute successfully");

    assert_eq!(output.code, Some(42));
    assert_eq!(output.stdout, "out");
    assert_eq!(output.stderr, "err");
}

#[rstest]
fn streaming_runner_propagates_non_zero_exit_code() {
    let runner = StreamingCommandRunner;
    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from("exit 7")])
        .expect("command should execute successfully");

    assert_eq!(output.code, Some(7));
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");
}

#[rstest]
fn streaming_runner_handles_no_output() {
    let runner = StreamingCommandRunner;
    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from("")])
        .expect("command should execute successfully");

    assert_eq!(output.code, Some(0));
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");
}

#[rstest]
fn streaming_runner_captures_large_interleaved_output() {
    let runner = StreamingCommandRunner;
    let output = runner
        .run(
            "sh",
            &[
                OsString::from("-c"),
                OsString::from(
                    "for i in $(seq 1 50); do printf \"out-%03d\\n\" $i; printf \"err-%03d\\n\" $i 1>&2; done",
                ),
            ],
        )
        .expect("command should execute successfully");

    let mut expected_out = String::new();
    let mut expected_err = String::new();
    for i in 1..=50 {
        writeln!(&mut expected_out, "out-{i:03}").expect("write expected_out");
        writeln!(&mut expected_err, "err-{i:03}").expect("write expected_err");
    }

    assert_eq!(output.code, Some(0));
    assert_eq!(output.stdout, expected_out);
    assert_eq!(output.stderr, expected_err);
}

#[rstest]
fn streaming_runner_failed_spawn_returns_spawn_error() {
    let runner = StreamingCommandRunner;
    let result = runner.run("definitely-not-a-real-binary-xyz", &[]);

    match result {
        Err(SyncError::Spawn { .. }) => {}
        other => panic!("expected SyncError::Spawn, got {other:?}"),
    }
}
