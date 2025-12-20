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
mod main_tests;

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
    /// Provide cloud-init user-data inline for this run (cloud-config YAML or script).
    ///
    /// This payload is passed through to the backend and applied during the
    /// instance's first boot before the remote command is executed.
    #[arg(long, value_name = "USER_DATA", conflicts_with = "cloud_init_file")]
    cloud_init: Option<String>,
    /// Provide cloud-init user-data from a local file for this run.
    #[arg(long, value_name = "PATH", conflicts_with = "cloud_init")]
    cloud_init_file: Option<String>,
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
    #[error("invalid cloud-init configuration: {0}")]
    InvalidCloudInit(String),
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

    let (backend, request) = build_backend_and_request(&args)?;

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

fn build_backend_and_request(
    args: &RunCommand,
) -> Result<(ScalewayBackend, InstanceRequest), CliError> {
    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let backend =
        ScalewayBackend::new(scaleway_config).map_err(|err| CliError::Backend(err.to_string()))?;
    let mut request = backend
        .default_request()
        .map_err(|err| CliError::Backend(err.to_string()))?;
    apply_instance_overrides(&mut request, args)?;
    Ok((backend, request))
}

fn apply_instance_overrides(
    request: &mut InstanceRequest,
    args: &RunCommand,
) -> Result<(), CliError> {
    if let Some(instance_type) = args.instance_type.as_deref() {
        request.instance_type = parse_override("--instance-type", instance_type)?;
    }

    if let Some(image) = args.image.as_deref() {
        request.image_label = parse_override("--image", image)?;
    }

    if args.cloud_init.is_some() || args.cloud_init_file.is_some() {
        request.cloud_init_user_data = resolve_cloud_init_for_run(args)?;
    }

    Ok(())
}

fn resolve_cloud_init_for_run(args: &RunCommand) -> Result<Option<String>, CliError> {
    mriya::cloud_init::resolve_cloud_init_user_data(
        args.cloud_init.as_deref(),
        args.cloud_init_file.as_deref(),
    )
    .map_err(|err| match err {
        mriya::cloud_init::CloudInitError::BothProvided => CliError::InvalidCloudInit(
            String::from("provide only one of --cloud-init or --cloud-init-file"),
        ),
        mriya::cloud_init::CloudInitError::InlineEmpty => {
            CliError::InvalidCloudInit(String::from("--cloud-init must not be empty or whitespace"))
        }
        mriya::cloud_init::CloudInitError::FilePathEmpty => CliError::InvalidCloudInit(
            String::from("--cloud-init-file must not be empty or whitespace"),
        ),
        mriya::cloud_init::CloudInitError::FileEmpty => {
            CliError::InvalidCloudInit(String::from("--cloud-init-file must not be empty"))
        }
        mriya::cloud_init::CloudInitError::FileRead { path, message } => {
            CliError::InvalidCloudInit(format!(
                "failed to read --cloud-init-file {path}: {message}"
            ))
        }
    })
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
    let (_backend, request) = build_backend_and_request(args)?;

    writeln!(io::stdout(), "instance_type={}", request.instance_type).ok();
    writeln!(io::stdout(), "image_label={}", request.image_label).ok();
    writeln!(
        io::stdout(),
        "cloud_init_user_data_present={}",
        request.cloud_init_user_data.is_some()
    )
    .ok();
    writeln!(
        io::stdout(),
        "cloud_init_user_data_size={}",
        request.cloud_init_user_data.as_deref().map_or(0, str::len)
    )
    .ok();
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
