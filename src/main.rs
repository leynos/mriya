//! Binary entry point for the Mriya CLI.

use std::env;
use std::io::{self, Write};
use std::process;
#[cfg(test)]
use std::sync::OnceLock;
#[cfg(test)]
use std::{future::Future, pin::Pin};

use camino::Utf8PathBuf;
use clap::Parser;
use shell_escape::unix::escape;
use thiserror::Error;

use mriya::{
    RunError, RunOrchestrator, ScalewayBackend, ScalewayBackendError, ScalewayConfig,
    StreamingCommandRunner, SyncConfig, Syncer,
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
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let exit_code = match dispatch(cli).await {
        Ok(code) => code,
        Err(err) => {
            report_error(&err);
            1
        }
    };

    process::exit(exit_code);
}

async fn dispatch(cli: Cli) -> Result<i32, CliError> {
    match cli {
        Cli::Run(command) => {
            #[cfg(test)]
            if let Some(hook) = RUN_COMMAND_HOOK.get() {
                return hook(command).await;
            }

            run_command(command).await
        }
    }
}

async fn run_command(args: RunCommand) -> Result<i32, CliError> {
    if let Some(result) = fake_run_from_env(&args) {
        return result;
    }

    if let Some(err) = prefail_from_env() {
        return Err(err);
    }

    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let backend =
        ScalewayBackend::new(scaleway_config).map_err(|err| CliError::Backend(err.to_string()))?;
    let request = backend
        .default_request()
        .map_err(|err| CliError::Backend(err.to_string()))?;

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
            .any(|ch| matches!(ch, '\n' | '\r' | '\u{0000}'..='\u{001F}' | '\u{007F}'))
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
static RUN_COMMAND_HOOK: OnceLock<Box<RunHook>> = OnceLock::new();

fn fake_run_from_env(args: &RunCommand) -> Option<Result<i32, CliError>> {
    let mode = env::var("MRIYA_FAKE_RUN_MODE").ok()?;
    let _ = args; // suppress unused warning when compiled without tests hitting this path
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
        "missing-exit" => {
            writeln!(io::stdout(), "fake-stdout").ok();
            writeln!(io::stderr(), "fake-stderr").ok();
            Some(Err(CliError::MissingExitCode))
        }
        _ => None,
    }
}

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

    async fn dispatch_with_hook<F, Fut>(hook: F) -> Result<i32, CliError>
    where
        F: Fn(RunCommand) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<i32, CliError>> + Send + 'static,
    {
        RUN_COMMAND_HOOK
            .set(Box::new(move |cmd| Box::pin(hook(cmd))))
            .ok();
        let cli = Cli::Run(RunCommand {
            command: vec![String::from("echo")],
        });
        dispatch(cli).await
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

    #[tokio::test]
    async fn run_command_prefail_variants() {
        type ErrorPredicate = fn(&CliError) -> bool;
        let cases: [(&str, ErrorPredicate); 4] = [
            ("config", |err: &CliError| {
                matches!(err, CliError::Config(_))
            }),
            ("sync", |err: &CliError| matches!(err, CliError::Sync(_))),
            ("backend", |err: &CliError| {
                matches!(err, CliError::Backend(_))
            }),
            ("run", |err: &CliError| matches!(err, CliError::Run(_))),
        ];

        for (mode, predicate) in cases {
            let _guard = EnvGuard::set_var("MRIYA_FAKE_RUN_PREFAIL", mode).await;
            let result = run_command(RunCommand {
                command: vec![String::from("echo")],
            })
            .await;
            let err = result.expect_err("prefail should error");
            assert!(
                predicate(&err),
                "mode {mode} produced unexpected error: {err}"
            );
        }
    }

    #[tokio::test]
    async fn run_command_missing_exit_code_from_fake_mode() {
        let _guard = EnvGuard::set_var("MRIYA_FAKE_RUN_MODE", "missing-exit").await;
        let result = run_command(RunCommand {
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
