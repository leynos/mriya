//! Tests for `StreamingCommandRunner` output forwarding and capture.

use super::super::*;
use rstest::rstest;
use std::ffi::OsString;
use std::fmt::Write as _;

/// Helper to run a shell script via `StreamingCommandRunner` and assert expected output.
fn assert_streaming_runner_output(
    script: &str,
    expected_code: Option<i32>,
    expected_stdout: &str,
    expected_stderr: &str,
) {
    let runner = StreamingCommandRunner;
    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from(script)])
        .expect("command should execute successfully");

    assert_eq!(output.code, expected_code);
    assert_eq!(output.stdout, expected_stdout);
    assert_eq!(output.stderr, expected_stderr);
}

#[rstest]
fn streaming_runner_captures_output() {
    assert_streaming_runner_output("printf out && printf err 1>&2", Some(0), "out", "err");
}

#[rstest]
fn streaming_runner_captures_output_on_failure() {
    assert_streaming_runner_output(
        "printf out && printf err 1>&2; exit 42",
        Some(42),
        "out",
        "err",
    );
}

#[rstest]
fn streaming_runner_propagates_non_zero_exit_code() {
    let runner = StreamingCommandRunner;
    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from("exit 7")])
        .expect("command should execute successfully");

    assert_eq!(output.code, Some(7));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[rstest]
fn streaming_runner_handles_no_output() {
    let runner = StreamingCommandRunner;
    let output = runner
        .run("sh", &[OsString::from("-c"), OsString::from("")])
        .expect("command should execute successfully");

    assert_eq!(output.code, Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
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
