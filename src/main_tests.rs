//! Unit tests for the `mriya` CLI binary implementation.
//!
//! Keeping these tests in a separate module helps keep `src/main.rs` focused
//! and within the repository's file size limits.

use super::*;
use rstest::rstest;

async fn dispatch_with_hook<F, Fut>(hook: F) -> Result<i32, CliError>
where
    F: Fn(RunCommand) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<i32, CliError>> + Send + 'static,
{
    *RUN_COMMAND_HOOK.lock().await = Some(Box::new(move |cmd| Box::pin(hook(cmd))));
    let result = exec_run(RunCommand {
        instance_type: None,
        image: None,
        cloud_init: None,
        cloud_init_file: None,
        command: vec![String::from("echo")],
    })
    .await;
    // Clear the hook after use to prevent interference with other tests
    *RUN_COMMAND_HOOK.lock().await = None;
    result
}

#[test]
fn validate_command_args_rejects_control_characters() {
    let err =
        validate_command_args(&[String::from("echo\tbad")]).expect_err("tab should be rejected");

    assert!(
        matches!(err, CliError::InvalidCommand(ref message) if message.contains("control characters")),
        "unexpected error: {err}"
    );
}

#[test]
fn validate_command_args_accepts_safe_arguments() {
    assert!(validate_command_args(&[String::from("echo"), String::from("ok")]).is_ok());
}

#[test]
fn render_remote_command_escapes_arguments() {
    let args = vec![
        String::from("echo"),
        String::from("a b"),
        String::from("c'd"),
    ];
    let rendered = render_remote_command(&args);

    assert_eq!(rendered, "echo 'a b' 'c'\\''d'");
}

fn mock_env(pairs: &[(&str, &str)]) -> mockable::MockEnv {
    let table: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    let mut env = mockable::MockEnv::new();
    env.expect_string()
        .returning(move |key| table.get(key).cloned());
    env
}

fn echo_command() -> RunCommand {
    RunCommand {
        instance_type: None,
        image: None,
        cloud_init: None,
        cloud_init_file: None,
        command: vec![String::from("echo")],
    }
}

#[rstest]
#[case::config("config", |err: &CliError| matches!(err, CliError::Config(_)))]
#[case::sync("sync", |err: &CliError| matches!(err, CliError::Sync(_)))]
#[case::backend("backend", |err: &CliError| matches!(err, CliError::Backend(_)))]
#[case::run("run", |err: &CliError| matches!(err, CliError::Run(_)))]
fn fake_outcome_prefail_variants(#[case] mode: &str, #[case] predicate: fn(&CliError) -> bool) {
    let env = mock_env(&[
        ("MRIYA_FAKE_RUN_ENABLE", "1"),
        ("MRIYA_FAKE_RUN_PREFAIL", mode),
    ]);
    let outcome = test_backdoors::select_fake_outcome(&env).expect("prefail should be selected");
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let err = test_backdoors::run_fake_outcome(&echo_command(), outcome, &mut stdout, &mut stderr)
        .expect_err("prefail should error");
    assert!(
        predicate(&err),
        "mode {mode} produced unexpected error: {err}"
    );
}

#[test]
fn fake_outcome_missing_exit_code_from_fake_mode() {
    let env = mock_env(&[
        ("MRIYA_FAKE_RUN_ENABLE", "1"),
        ("MRIYA_FAKE_RUN_MODE", "missing-exit"),
    ]);
    let outcome =
        test_backdoors::select_fake_outcome(&env).expect("missing-exit should be selected");
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let result =
        test_backdoors::run_fake_outcome(&echo_command(), outcome, &mut stdout, &mut stderr);

    assert!(
        matches!(result, Err(CliError::MissingExitCode)),
        "expected MissingExitCode, got {result:?}"
    );
}

#[test]
fn fake_outcome_prefers_a_run_mode_over_a_prefail_mode() {
    // Both modes are selected; a fake run models a completed run, so it must
    // win over the early-failure mode rather than depending on match order.
    let env = mock_env(&[
        ("MRIYA_FAKE_RUN_ENABLE", "1"),
        ("MRIYA_FAKE_RUN_MODE", "exit-0"),
        ("MRIYA_FAKE_RUN_PREFAIL", "backend"),
    ]);

    let outcome = test_backdoors::select_fake_outcome(&env)
        .expect("a selected fake run should short-circuit the real path");
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let exit_code =
        test_backdoors::run_fake_outcome(&echo_command(), outcome, &mut stdout, &mut stderr)
            .expect("a selected fake run should report an exit code");

    assert_eq!(exit_code, 0, "the run mode should take precedence");
}

#[test]
fn fake_run_outcome_writes_the_streamed_output_it_simulates() {
    // The outcome owns its streams, so a test can assert exactly what a fake
    // run emits without capturing the harness's own stdout and stderr.
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());

    let exit_code = test_backdoors::run_fake_outcome(
        &echo_command(),
        test_backdoors::FakeOutcome::Run(test_backdoors::FakeRunMode::Exit7),
        &mut stdout,
        &mut stderr,
    )
    .expect("exit-7 reports an exit code rather than an error");

    assert_eq!(String::from_utf8_lossy(&stdout), "fake-stdout\n");
    assert_eq!(String::from_utf8_lossy(&stderr), "fake-stderr\n");
    assert_eq!(exit_code, 7);
}

#[rstest]
#[case::disabled(&[("MRIYA_FAKE_RUN_MODE", "exit-0")])]
#[case::enable_not_one(&[("MRIYA_FAKE_RUN_ENABLE", "0"), ("MRIYA_FAKE_RUN_MODE", "exit-0")])]
#[case::no_mode(&[("MRIYA_FAKE_RUN_ENABLE", "1")])]
fn fake_outcome_is_none_unless_enabled_and_selected(#[case] vars: &[(&str, &str)]) {
    let env = mock_env(vars);
    assert!(test_backdoors::select_fake_outcome(&env).is_none());
}

#[tokio::test]
async fn dispatch_uses_hook_result() {
    let result = dispatch_with_hook(|_| async { Ok(42) }).await;
    assert!(matches!(result, Ok(42)));
}

#[test]
fn parse_override_trims_and_accepts_nonempty_values() {
    let parsed = parse_override("--instance-type", "  DEV1-M  ")
        .expect("non-empty instance type override should parse");
    assert_eq!(parsed, "DEV1-M");
}

#[test]
fn parse_override_rejects_empty_or_whitespace_values() {
    let err = parse_override("--image", "   ").expect_err("whitespace override should fail");
    assert!(
        matches!(
            err,
            CliError::InvalidOverride {
                field: "--image",
                ..
            }
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn apply_instance_overrides_updates_request() {
    let mut request = InstanceRequest::builder()
        .image_label("Ubuntu 24.04 Noble Numbat")
        .instance_type("DEV1-S")
        .zone("fr-par-1")
        .project_id("project")
        .architecture("x86_64")
        .build()
        .expect("base request should build");

    let args = RunCommand {
        instance_type: Some(String::from("  DEV1-M  ")),
        image: Some(String::from("  ubuntu-22-04  ")),
        cloud_init: None,
        cloud_init_file: None,
        command: vec![String::from("echo"), String::from("ok")],
    };

    apply_instance_overrides(&mut request, &args).expect("valid instance overrides should apply");

    assert_eq!(request.instance_type, "DEV1-M");
    assert_eq!(request.image_label, "ubuntu-22-04");
}

#[test]
fn write_error_writes_cli_error() {
    let mut buf = Vec::new();
    let err = CliError::MissingExitCode;
    write_error(&mut buf, &err);
    let rendered = String::from_utf8(buf).expect("utf8");
    assert!(
        rendered.contains("remote command terminated without an exit status"),
        "rendered: {rendered}"
    );
}
