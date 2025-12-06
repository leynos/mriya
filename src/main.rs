//! Binary entry point for the Mriya CLI.

use std::io::{self, Write};
use std::process;

use camino::Utf8PathBuf;
use clap::Parser;
use shell_escape::unix::escape;
use thiserror::Error;

use mriya::config::ConfigError;
use mriya::{
    RunError, RunOrchestrator, ScalewayBackend, ScalewayBackendError, ScalewayConfig,
    StreamingCommandRunner, SyncConfig, SyncConfigLoadError, SyncError, Syncer,
};

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
    #[error("failed to load Scaleway configuration: {0}")]
    LoadScaleway(ConfigError),
    #[error("failed to load sync configuration: {0}")]
    LoadSync(SyncConfigLoadError),
    #[error("failed to build sync orchestrator: {0}")]
    SyncSetup(SyncError),
    #[error("failed to build Scaleway backend: {0}")]
    Backend(ScalewayBackendError),
    #[error("failed to read current working directory: {0}")]
    WorkingDir(std::io::Error),
    #[error("current working directory is not valid UTF-8: {0}")]
    NonUtf8Path(String),
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
        Cli::Run(command) => run_command(command).await,
    }
}

async fn run_command(args: RunCommand) -> Result<i32, CliError> {
    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(CliError::LoadScaleway)?;
    let backend = ScalewayBackend::new(scaleway_config).map_err(CliError::Backend)?;
    let request = backend.default_request().map_err(CliError::Backend)?;

    let sync_config = SyncConfig::load_without_cli_args().map_err(CliError::LoadSync)?;
    let syncer = Syncer::new(sync_config, StreamingCommandRunner).map_err(CliError::SyncSetup)?;

    let cwd = std::env::current_dir().map_err(CliError::WorkingDir)?;
    let source = Utf8PathBuf::from_path_buf(cwd)
        .map_err(|path| CliError::NonUtf8Path(path.display().to_string()))?;

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
            return Err(CliError::InvalidCommand(String::from(
                "command arguments must not contain control characters (ASCII \
                 0x00-0x1F or 0x7F, e.g. newline, carriage return, tab, NUL)",
            )));
        }
    }
    Ok(())
}

fn report_error(err: &CliError) {
    let mut stderr = io::stderr();
    if writeln!(stderr, "{err}").is_err() {}
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
