//! CLI entry point for Mriya.
//!
//! This binary provisions a short-lived Scaleway instance, synchronises the
//! local workspace via `rsync`, executes a user-supplied command over SSH, and
//! tears the instance down. The `run` subcommand preserves remote exit codes
//! locally and reports errors on stderr with meaningful exit statuses.

#[cfg(any(test, feature = "test-backdoors"))]
use std::env;
use std::io::{self, Write};
use std::process;
#[cfg(test)]
use std::{future::Future, pin::Pin};
#[cfg(test)]
use tokio::sync::Mutex;

use camino::Utf8PathBuf;
use clap::Parser;
use shell_escape::unix::escape;
use thiserror::Error;

use mriya::{
    InstanceRequest, RunError, RunOrchestrator, ScalewayBackend, ScalewayBackendError,
    ScalewayConfig, StreamingCommandRunner, SyncConfig, Syncer,
};

#[cfg(test)]
mod test_helpers;

#[derive(Debug, Parser)]
#[command(
    name = "mriya",
    about = "Teleport your workspace to a Scaleway VM and run commands remotely",
    arg_required_else_help = true
)]
enum Cli {
    #[command(name = "run", about = "Provision, sync, and run a command over SSH")]
    Run(RunCommand),
}

#[derive(Debug, Parser)]
struct RunCommand {
    /// Override the Scaleway instance type (commercial type) for this run.
    ///
    /// The Scaleway backend validates availability in the selected zone during
    /// provisioning, and rejects unknown values with a provider-specific
    /// error.
    #[arg(long, value_name = "TYPE")]
    instance_type: Option<String>,
    /// Override the image label for this run.
    ///
    /// The Scaleway backend resolves the label to a concrete image identifier
    /// for the selected architecture and zone, and rejects unknown labels with
    /// a provider-specific error.
    #[arg(long, value_name = "IMAGE")]
    image: Option<String>,
    /// Command to execute on the remote host (use -- to separate flags).
    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

#[derive(Debug, Error)]
enum CliError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("sync error: {0}")]
    Sync(String),
    #[error("remote command terminated without an exit status")]
    MissingExitCode,
    #[error("remote run failed: {0}")]
    Run(#[from] RunError<ScalewayBackendError>),
    #[error("invalid command argument: {0}")]
    InvalidCommand(String),
    #[error("invalid override for {field}: {message}")]
    InvalidOverride {
        field: &'static str,
        message: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let exit_code = match cli {
        Cli::Run(command) => exec_run(command).await,
    }
    .unwrap_or_else(|err| {
        report_error(&err);
        1
    });

    process::exit(exit_code);
}

async fn exec_run(command: RunCommand) -> Result<i32, CliError> {
    #[cfg(test)]
    if let Some(hook) = RUN_COMMAND_HOOK.lock().await.as_ref() {
        return hook(command).await;
    }

    run_command(command).await
}

async fn run_command(args: RunCommand) -> Result<i32, CliError> {
    #[cfg(any(test, feature = "test-backdoors"))]
    {
        if enable_fake_modes() {
            if let Some(result) = fake_run_from_env(&args) {
                return result;
            }

            if let Some(err) = prefail_from_env() {
                return Err(err);
            }
        }
    }

    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let backend =
        ScalewayBackend::new(scaleway_config).map_err(|err| CliError::Backend(err.to_string()))?;
    let mut request = backend
        .default_request()
        .map_err(|err| CliError::Backend(err.to_string()))?;
    apply_instance_overrides(&mut request, &args)?;

    let sync_config =
        SyncConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let syncer = Syncer::new(sync_config, StreamingCommandRunner)
        .map_err(|err| CliError::Sync(err.to_string()))?;

    let cwd = std::env::current_dir().map_err(|err| CliError::Config(err.to_string()))?;
    let source = Utf8PathBuf::from_path_buf(cwd)
        .map_err(|path| CliError::Config(path.display().to_string()))?;

    let orchestrator = RunOrchestrator::new(backend, syncer);
    validate_command_args(&args.command)?;
    let remote_command = render_remote_command(&args.command);
    let output = orchestrator
        .execute(&request, &source, &remote_command)
        .await?;

    output.exit_code.ok_or(CliError::MissingExitCode)
}

fn apply_instance_overrides(
    request: &mut InstanceRequest,
    args: &RunCommand,
) -> Result<(), CliError> {
    if let Some(instance_type) = args.instance_type.as_deref() {
        request.instance_type = parse_override("instance_type", instance_type)?;
    }

    if let Some(image) = args.image.as_deref() {
        request.image_label = parse_override("image", image)?;
    }

    Ok(())
}

fn parse_override(field: &'static str, value: &str) -> Result<String, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::InvalidOverride {
            field,
            message: String::from("must not be empty or whitespace"),
        });
    }
    Ok(trimmed.to_owned())
}

fn render_remote_command(args: &[String]) -> String {
    let mut result = String::new();
    let mut first = true;

    for arg in args {
        if first {
            first = false;
        } else {
            result.push(' ');
        }

        let escaped = escape(arg.as_str().into());
        result.push_str(escaped.as_ref());
    }

    result
}

fn validate_command_args(args: &[String]) -> Result<(), CliError> {
    for arg in args {
        if arg
            .chars()
            .any(|ch| matches!(ch, '\u{0000}'..='\u{001F}' | '\u{007F}'))
        {
            return Err(CliError::InvalidCommand(String::from(concat!(
                "command arguments must not contain control characters (ASCII ",
                "0x00-0x1F or 0x7F, e.g. newline, carriage return, tab, NUL)"
            ))));
        }
    }
    Ok(())
}

fn report_error(err: &CliError) {
    write_error(io::stderr(), err);
}

fn write_error(mut target: impl Write, err: &CliError) {
    writeln!(target, "{err}").ok();
}

#[cfg(test)]
type RunHook =
    dyn Fn(RunCommand) -> Pin<Box<dyn Future<Output = Result<i32, CliError>> + Send>> + Send + Sync;

#[cfg(test)]
static RUN_COMMAND_HOOK: Mutex<Option<Box<RunHook>>> = Mutex::const_new(None);

#[cfg(any(test, feature = "test-backdoors"))]
fn enable_fake_modes() -> bool {
    env::var("MRIYA_FAKE_RUN_ENABLE")
        .map(|val| val == "1")
        .unwrap_or(false)
}

#[cfg(any(test, feature = "test-backdoors"))]
fn fake_run_from_env(args: &RunCommand) -> Option<Result<i32, CliError>> {
    let mode = env::var("MRIYA_FAKE_RUN_MODE").ok()?;
    match mode.as_str() {
        "exit-0" => {
            writeln!(io::stdout(), "fake-stdout").ok();
            writeln!(io::stderr(), "fake-stderr").ok();
            Some(Ok(0))
        }
        "exit-7" => {
            writeln!(io::stdout(), "fake-stdout").ok();
            writeln!(io::stderr(), "fake-stderr").ok();
            Some(Ok(7))
        }
        "dump-request" => Some(fake_dump_request(args)),
        "missing-exit" => {
            writeln!(io::stdout(), "fake-stdout").ok();
            writeln!(io::stderr(), "fake-stderr").ok();
            Some(Err(CliError::MissingExitCode))
        }
        _ => None,
    }
}

#[cfg(any(test, feature = "test-backdoors"))]
fn fake_dump_request(args: &RunCommand) -> Result<i32, CliError> {
    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let backend =
        ScalewayBackend::new(scaleway_config).map_err(|err| CliError::Backend(err.to_string()))?;
    let mut request = backend
        .default_request()
        .map_err(|err| CliError::Backend(err.to_string()))?;
    apply_instance_overrides(&mut request, args)?;

    writeln!(io::stdout(), "instance_type={}", request.instance_type).ok();
    writeln!(io::stdout(), "image_label={}", request.image_label).ok();
    Ok(0)
}

#[cfg(any(test, feature = "test-backdoors"))]
fn prefail_from_env() -> Option<CliError> {
    let mode = env::var("MRIYA_FAKE_RUN_PREFAIL").ok()?;
    match mode.as_str() {
        "config" => Some(CliError::Config(String::from("fake"))),
        "sync" => Some(CliError::Sync(String::from("fake"))),
        "backend" => Some(CliError::Backend(String::from("fake"))),
        "run" => Some(CliError::Run(RunError::Provision(
            ScalewayBackendError::Config(String::from("fake")),
        ))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
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
        let err = validate_command_args(&[String::from("echo\tbad")])
            .expect_err("tab should be rejected");

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
        let parsed = parse_override("instance_type", "  DEV1-M  ")
            .unwrap_or_else(|err| panic!("expected override to parse: {err}"));
        assert_eq!(parsed, "DEV1-M");
    }

    #[test]
    fn parse_override_rejects_empty_or_whitespace_values() {
        let err = parse_override("image", "   ").expect_err("whitespace override should fail");
        assert!(
            matches!(err, CliError::InvalidOverride { field: "image", .. }),
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
}
