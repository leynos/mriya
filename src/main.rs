//! CLI entry point for Mriya.
//!
//! This binary provisions a short-lived Scaleway instance, synchronizes the
//! local workspace via `rsync`, executes a user-supplied command over SSH, and
//! tears the instance down. The `run` subcommand preserves remote exit codes
//! locally and reports errors on stderr with meaningful exit statuses.

#[cfg(test)]
use std::{future::Future, pin::Pin};
use std::{
    io::{self, Write},
    process,
};

use camino::Utf8PathBuf;
use clap::Parser;
use thiserror::Error;
#[cfg(test)]
use tokio::sync::Mutex;

mod cli;
#[cfg(test)]
mod test_backdoors;

use cli::{Cli, InitCommand, RunCommand};
use mriya::{
    ConfigStore,
    InitConfig,
    InitError,
    InitOrchestrator,
    InitRequest,
    InstanceRequest,
    RunError,
    RunOrchestrator,
    ScalewayBackend,
    ScalewayBackendError,
    ScalewayConfig,
    StreamingCommandRunner,
    SyncConfig,
    Syncer,
    environment_volume_id,
};
#[cfg(test)]
use test_backdoors::{run_fake_outcome, select_fake_outcome};

mod cli_args;
#[cfg(test)]
mod main_request_tests;
#[cfg(test)]
mod main_tests;

use cli_args::{parse_override, render_remote_command, validate_command_args};

/// Failure modes of a CLI invocation, each rendered as one line on stderr.
///
/// Every variant leads to exit status 1; only a remote command's own exit
/// code is passed through as the process status.
#[derive(Debug, Error)]
enum CliError {
    /// Configuration could not be loaded, or the working directory could not be
    /// determined as UTF-8.
    #[error("configuration error: {0}")]
    Config(String),
    /// The backend could not be constructed or could not build a default request.
    #[error("backend error: {0}")]
    Backend(String),
    /// The syncer could not be constructed from its configuration.
    #[error("sync error: {0}")]
    Sync(String),
    /// The remote command finished without reporting an exit code, such as when
    /// it was terminated by a signal.
    #[error("remote command terminated without an exit status")]
    MissingExitCode,
    /// The run workflow failed after validation, from provisioning to teardown.
    #[error("remote run failed: {0}")]
    Run(#[from] RunError<ScalewayBackendError>),
    /// The remote command contains control characters that cannot be passed
    /// safely to the remote shell.
    #[error("invalid command argument: {0}")]
    InvalidCommand(String),
    /// A command-line override was empty or whitespace.
    #[error("invalid override for {field}: {message}")]
    InvalidOverride {
        /// Flag that was overridden, such as `--instance-type`.
        field: &'static str,
        /// Why the value was rejected.
        message: String,
    },
    /// The cloud-init flags were conflicting, empty, or named an unreadable file.
    #[error("invalid cloud-init configuration: {0}")]
    InvalidCloudInit(String),
    /// The `init` workflow failed.
    #[error("init failed: {0}")]
    Init(#[from] InitError<ScalewayBackendError>),
}

fn main() {
    // Build the runtime explicitly so runtime construction errors are
    // reported through the usual error path instead of panicking inside the
    // `#[tokio::main]` expansion.
    let exit_code = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(async_main()),
        Err(err) => {
            writeln!(io::stderr(), "failed to start async runtime: {err}").ok();
            1
        }
    };

    process::exit(exit_code);
}

/// Parses arguments, dispatches the subcommand, and maps the outcome to a
/// process exit status.
///
/// Errors are reported on stderr and become status 1.
///
/// For example, a command that exits 7 yields 7, while an `init` failure
/// prints its message and yields 1.
async fn async_main() -> i32 {
    let cli = Cli::parse();
    match cli {
        Cli::Run(command) => exec_run(command).await,
        Cli::Init(command) => exec_init(command).await,
    }
    .unwrap_or_else(|err| {
        report_error(&err);
        1
    })
}

/// Runs the `run` subcommand.
///
/// In unit tests a hook installed in `RUN_COMMAND_HOOK` takes precedence, so
/// the dispatch can be tested without provisioning anything.
///
/// For example, with no hook installed the command is handed to
/// [`run_command`], whose exit status becomes the process status.
async fn exec_run(command: RunCommand) -> Result<i32, CliError> {
    #[cfg(test)]
    if let Some(hook) = RUN_COMMAND_HOOK.lock().await.as_ref() {
        return hook(command).await;
    }

    run_command(command).await
}

/// Runs the `init` subcommand.
///
/// For example, a successful init yields 0, while an invalid configuration
/// yields [`CliError::Config`] for the caller to report.
async fn exec_init(command: InitCommand) -> Result<i32, CliError> { init_command(command).await }

/// Provisions an instance, syncs the workspace, runs the command remotely,
/// and returns the remote exit code.
///
/// In a unit-test build only, an outcome selected from the environment
/// short-circuits everything below so tests can assert exit codes and streamed
/// output without cloud access. That branch is compiled out of every other
/// build; no Cargo feature or profile can enable it. Command arguments are
/// validated after the backend and syncer have been constructed.
///
/// For example, a remote command exiting 7 yields 7, so the caller can pass
/// the remote status straight through as the process exit code.
///
/// # Errors
///
/// Returns the first failure from request construction, sync configuration,
/// the remote command, or teardown.
async fn run_command(args: RunCommand) -> Result<i32, CliError> {
    #[cfg(test)]
    {
        if let Some(outcome) = select_fake_outcome(&mockable::DefaultEnv) {
            let mut stdout = io::stdout();
            let mut stderr = io::stderr();
            return run_fake_outcome(&args, outcome, &mut stdout, &mut stderr);
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

/// Creates a cache volume for the current project and records it in the
/// configuration, printing the volume ID and configuration path.
///
/// The project name is the working directory's final component, or `mriya`
/// when there is none.
///
/// For example, running in `/home/ada/work` creates a cache volume named
/// `work` and reports success, while a provider rejection becomes the
/// corresponding [`CliError`].
async fn init_command(args: InitCommand) -> Result<i32, CliError> {
    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let init_config =
        InitConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let sync_config =
        SyncConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;

    let backend = ScalewayBackend::new(scaleway_config.clone())
        .map_err(|err| CliError::Backend(err.to_string()))?;
    let syncer = Syncer::new(sync_config, StreamingCommandRunner)
        .map_err(|err| CliError::Sync(err.to_string()))?;

    let cwd = std::env::current_dir().map_err(|err| CliError::Config(err.to_string()))?;
    let cwd_path = Utf8PathBuf::from_path_buf(cwd)
        .map_err(|path| CliError::Config(path.display().to_string()))?;
    let project_name = cwd_path.file_name().unwrap_or("mriya");

    // The environment overrides any file value on every run, so record it
    // separately; `init` refuses to replace it even with `--force`.
    let environment_volume_id = environment_volume_id(&mockable::DefaultEnv)
        .map_err(|err| CliError::Config(err.to_string()))?;
    let request =
        InitRequest::from_config(&scaleway_config, &init_config, project_name, args.force)
            .map_err(|err| CliError::Config(err.to_string()))?
            .with_environment_volume_id(environment_volume_id);

    let orchestrator = InitOrchestrator::new(backend, syncer, ConfigStore::new());
    let outcome = orchestrator.execute(&request).await?;

    writeln!(
        io::stdout(),
        "created cache volume {} and updated {}",
        outcome.volume_id,
        outcome.config_path
    )
    .ok();

    Ok(0)
}

/// Builds the Scaleway backend and its default instance request, then
/// applies the command-line overrides to that request.
///
/// For example, with no overrides the request carries the configured defaults,
/// while `--instance-type DEV1-M` replaces only the instance type.
fn build_backend_and_request(
    args: &RunCommand,
) -> Result<(ScalewayBackend, InstanceRequest), CliError> {
    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    build_backend_and_request_with(&scaleway_config, args, &mockable::DefaultEnv)
}

/// Builds the backend and its default instance request from configuration and
/// environment that have already been resolved, then applies the command-line
/// overrides.
///
/// Loading is split from building so a test can supply both inputs without
/// reading the process environment, matching `ScalewayConfig::as_request` and
/// `as_request_with_env`. The environment reaches every read on the way to the
/// request: `MRIYA_TEST_RUN_ID` for resource tagging during backend
/// construction, and `HOME` for a `~/` cloud-init path in both the configured
/// file and the command-line flag. For example, a test passes the documented
/// defaults and a `MockEnv` and asserts the request the binary would send,
/// while production passes what `ScalewayConfig::load_without_cli_args` and
/// `DefaultEnv` resolved.
fn build_backend_and_request_with(
    scaleway_config: &ScalewayConfig,
    args: &RunCommand,
    env: &impl mockable::Env,
) -> Result<(ScalewayBackend, InstanceRequest), CliError> {
    let backend = ScalewayBackend::new_with_env(scaleway_config.clone(), env)
        .map_err(|err| CliError::Backend(err.to_string()))?;
    let mut request = backend
        .default_request_with_env(env)
        .map_err(|err| CliError::Backend(err.to_string()))?;
    apply_instance_overrides_with(&mut request, args, env)?;
    Ok((backend, request))
}

/// Applies `--instance-type`, `--image`, and cloud-init flags to `request`.
///
/// Only flags that were supplied change the request. The cloud-init flags
/// replace any user-data from configuration.
///
/// For example, passing only `--image` changes the image label and leaves the
/// instance type untouched, while `--instance-type "  "` returns
/// [`CliError::InvalidOverride`] naming that flag.
fn apply_instance_overrides_with(
    request: &mut InstanceRequest,
    args: &RunCommand,
    env: &impl mockable::Env,
) -> Result<(), CliError> {
    if let Some(instance_type) = args.instance_type.as_deref() {
        request.instance_type = parse_override("--instance-type", instance_type)?;
    }

    if let Some(image) = args.image.as_deref() {
        request.image_label = parse_override("--image", image)?;
    }

    if args.cloud_init.is_some() || args.cloud_init_file.is_some() {
        request.cloud_init_user_data = resolve_cloud_init_for_run_with(args, env)?;
    }

    Ok(())
}

/// Resolves the cloud-init flags into user-data, rewording each failure in
/// terms of the CLI flag names.
///
/// A `~/` path expands through `env`, so the caller controls where `HOME`
/// comes from. For example, `--cloud-init "#cloud-config"` yields that
/// payload, while passing both `--cloud-init` and `--cloud-init-file` returns
/// [`CliError::InvalidCloudInit`] naming both flags.
fn resolve_cloud_init_for_run_with(
    args: &RunCommand,
    env: &impl mockable::Env,
) -> Result<Option<String>, CliError> {
    mriya::cloud_init::resolve_cloud_init_user_data_with_env(
        args.cloud_init.as_deref(),
        args.cloud_init_file.as_deref(),
        env,
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
        mriya::cloud_init::CloudInitError::HomeUnavailable { path } => {
            CliError::InvalidCloudInit(format!(
                concat!(
                    "cannot expand --cloud-init-file {path}: HOME is set to a value that is ",
                    "not valid UTF-8; set HOME to a UTF-8 directory or pass an absolute path"
                ),
                path = path
            ))
        }
    })
}

/// Writes `err` to standard error.
///
/// For example, a configuration failure appears on stderr as one line and the
/// process then exits 1, so a script can detect the failure from the status.
fn report_error(err: &CliError) { write_error(io::stderr(), err); }

/// Writes `err` as a single line to `target`, ignoring write failures so a
/// closed stream cannot mask the original error.
///
/// For example, writing to an open buffer appends `"<message>\n"`, while a
/// writer that fails consumes the error instead of panicking, so the caller
/// still reports the original failure and exits 1.
fn write_error(mut target: impl Write, err: &CliError) { writeln!(target, "{err}").ok(); }

#[cfg(test)]
type RunHook =
    dyn Fn(RunCommand) -> Pin<Box<dyn Future<Output = Result<i32, CliError>> + Send>> + Send + Sync;

#[cfg(test)]
static RUN_COMMAND_HOOK: Mutex<Option<Box<RunHook>>> = Mutex::const_new(None);
