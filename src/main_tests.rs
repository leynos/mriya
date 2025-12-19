//! Unit tests for the `mriya` CLI binary implementation.
//!
//! Keeping these tests in a separate module helps keep `src/main.rs` focused
//! and within the repository's file size limits.

use super::*;
use crate::test_helpers::EnvGuard;
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

#[rstest]
#[case::config("config", |err: &CliError| matches!(err, CliError::Config(_)))]
#[case::sync("sync", |err: &CliError| matches!(err, CliError::Sync(_)))]
#[case::backend("backend", |err: &CliError| matches!(err, CliError::Backend(_)))]
#[case::run("run", |err: &CliError| matches!(err, CliError::Run(_)))]
#[tokio::test(flavor = "current_thread")]
async fn run_command_prefail_variants(
    #[case] mode: &str,
    #[case] predicate: fn(&CliError) -> bool,
) {
    let _guard = EnvGuard::set_vars(&[
        ("MRIYA_FAKE_RUN_ENABLE", "1"),
        ("MRIYA_FAKE_RUN_PREFAIL", mode),
    ])
    .await;
    let result = run_command(RunCommand {
        instance_type: None,
        image: None,
        command: vec![String::from("echo")],
    })
    .await;
    let err = result.expect_err("prefail should error");
    assert!(
        predicate(&err),
        "mode {mode} produced unexpected error: {err}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_missing_exit_code_from_fake_mode() {
    let _guard = EnvGuard::set_vars(&[
        ("MRIYA_FAKE_RUN_ENABLE", "1"),
        ("MRIYA_FAKE_RUN_MODE", "missing-exit"),
    ])
    .await;
    let result = run_command(RunCommand {
        instance_type: None,
        image: None,
        command: vec![String::from("echo")],
    })
    .await;

    assert!(
        matches!(result, Err(CliError::MissingExitCode)),
        "expected MissingExitCode, got {result:?}"
    );
}

#[tokio::test]
async fn dispatch_uses_hook_result() {
    let result = dispatch_with_hook(|_| async { Ok(42) }).await;
    assert!(matches!(result, Ok(42)));
}

#[test]
fn parse_override_trims_and_accepts_nonempty_values() {
    let parsed = parse_override("--instance-type", "  DEV1-M  ")
        .unwrap_or_else(|err| panic!("expected override to parse: {err}"));
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
        command: vec![String::from("echo"), String::from("ok")],
    };

    apply_instance_overrides(&mut request, &args)
        .unwrap_or_else(|err| panic!("expected overrides to apply: {err}"));

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
