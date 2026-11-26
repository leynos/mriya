//! Regression tests for the `mriya run` CLI.
//!
//! Each case arms a retired `MRIYA_FAKE_RUN_*` control and runs the ordinary
//! binary — the one `cargo build --bin mriya` produces, with no features — in
//! an environment cleared of everything but the isolated values. The run must
//! fail at the fail-closed argument stop and print none of the output a fake
//! mode would produce.
//!
//! The controls used to be compiled in behind a Cargo feature, which put a
//! production backdoor one build flag away. This suite is what keeps them
//! gone: reintroducing one makes these cases pass silently through the fake
//! path instead of stopping, so the status and output assertions fail.

use rstest::rstest;

#[path = "common/isolated_cli.rs"]
mod isolated_cli;

#[path = "common/test_binary.rs"]
mod test_binary;

#[path = "common/test_constants.rs"]
mod test_constants;

/// The variable that armed the retired fake modes.
const FAKE_ENABLE_VARIABLE: &str = "MRIYA_FAKE_RUN_ENABLE";

/// The variable that selected a retired fake run mode.
const FAKE_MODE_VARIABLE: &str = "MRIYA_FAKE_RUN_MODE";

/// The variable that selected a retired early-failure stage.
const FAKE_PREFAIL_VARIABLE: &str = "MRIYA_FAKE_RUN_PREFAIL";

/// The diagnostic the fail-closed stop produces.
const STOPPED_DIAGNOSTIC: &str =
    "invalid command argument: command arguments must not contain control characters";

/// The substrings no isolated run may print.
///
/// A fake run mode printed the first two, a request dump printed
/// `instance_type=`, and a fake pre-failure printed `: fake`.
const RETIRED_FAKE_OUTPUTS: [&str; 4] = ["fake-stdout", "fake-stderr", "instance_type=", ": fake"];

/// Proves one retired fake control changes nothing about an ordinary run.
#[rstest]
#[case::mode_exit_0(FAKE_MODE_VARIABLE, "exit-0")]
#[case::mode_exit_7(FAKE_MODE_VARIABLE, "exit-7")]
#[case::mode_missing_exit(FAKE_MODE_VARIABLE, "missing-exit")]
#[case::mode_dump_request(FAKE_MODE_VARIABLE, "dump-request")]
#[case::prefail_config(FAKE_PREFAIL_VARIABLE, "config")]
#[case::prefail_sync(FAKE_PREFAIL_VARIABLE, "sync")]
#[case::prefail_backend(FAKE_PREFAIL_VARIABLE, "backend")]
#[case::prefail_run(FAKE_PREFAIL_VARIABLE, "run")]
fn cli_run_ignores_retired_fake_run_controls(#[case] variable: &str, #[case] value: &str) {
    let controls = [(FAKE_ENABLE_VARIABLE, "1"), (variable, value)];
    let environment = isolated_cli::RunEnvironment {
        default_instance_type: test_constants::DEFAULT_INSTANCE_TYPE,
        default_image: isolated_cli::DEFAULT_IMAGE_LABEL,
        home: None,
        variables: &controls,
    };
    let run = isolated_cli::isolated_run_command(&environment)
        .expect("the isolated mriya command should be created");
    let output = run
        .execute(&[])
        .expect("the isolated mriya run should complete");
    let captured = isolated_cli::CliOutput::from_process_output(output);

    assert_eq!(
        captured.status_code, 1,
        "a fake control must not change the exit status; stdout: {}; stderr: {}",
        captured.stdout, captured.stderr
    );
    assert!(
        captured.stderr.contains(STOPPED_DIAGNOSTIC),
        "the run should stop at the control-character check; stderr: {}",
        captured.stderr
    );
    for marker in RETIRED_FAKE_OUTPUTS {
        for (stream, text) in [("stdout", &captured.stdout), ("stderr", &captured.stderr)] {
            assert!(
                !text.contains(marker),
                "a retired fake mode must not print `{marker}` on {stream}: {text}"
            );
        }
    }
}
