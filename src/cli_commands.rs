//! The `run` and `init` subcommand implementations.
//!
//! These functions build the backend, request, syncer, and ownership ledger,
//! then hand the work to an orchestrator. Keeping them out of `main.rs` leaves
//! the entry point focused on dispatch, exit codes, and error reporting.

use std::io::{self, Write};

use camino::Utf8PathBuf;
use mriya::{
    AllocationScope,
    Cancellation,
    ConfigStore,
    InitConfig,
    InitOrchestrator,
    InitRequest,
    InstanceRequest,
    RunOrchestrator,
    ScalewayBackend,
    ScalewayConfig,
    StreamingCommandRunner,
    SyncConfig,
    Syncer,
};

#[cfg(test)]
use crate::test_backdoors::{run_fake_outcome, select_fake_outcome};
use crate::{
    CliError,
    cli::{InitCommand, RunCommand},
    cli_args::{parse_override, render_remote_command, validate_command_args},
    cli_state,
};

/// Provisions an instance, runs the requested command, and cleans it up.
///
/// Short-circuits to a fake outcome when a test backdoor selects one, so the
/// CLI behaviour suite can assert exit codes without cloud access.
///
/// # Errors
///
/// Returns the first failure from request construction, sync configuration,
/// the remote command, or cleanup.
///
/// For example, a remote command exiting 7 yields 7, so the caller can pass
/// the remote status straight through as the process exit code.
///
/// In a unit-test build only, environment-selected fake modes can
/// short-circuit everything below; that branch is compiled out of every other
/// build. Command arguments are validated after the backend and syncer have
/// been constructed.
pub(crate) async fn run_command(
    args: RunCommand,
    cancellation: &Cancellation,
) -> Result<i32, CliError> {
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
    let syncer = Syncer::new(
        sync_config,
        StreamingCommandRunner::new(cancellation.scope()),
    )
    .map_err(|err| CliError::Sync(err.to_string()))?;

    let cwd = std::env::current_dir().map_err(|err| CliError::Config(err.to_string()))?;
    let source = Utf8PathBuf::from_path_buf(cwd)
        .map_err(|path| CliError::Config(path.display().to_string()))?;

    cli_state::warn_about_pending_journals();
    let ledger = cli_state::open_ledger(AllocationScope::new(
        request.zone.clone(),
        request.project_id.clone(),
    ))?;

    let orchestrator = RunOrchestrator::new(backend, syncer);
    validate_command_args(&args.command)?;
    let remote_command = render_remote_command(&args.command);
    let outcome = orchestrator
        .execute(mriya::run::RunInputs {
            request: &request,
            ledger: &ledger,
            source: &source,
            remote_command: &remote_command,
            cancellation,
        })
        .await?;

    report_warnings(&outcome.warnings);
    outcome.output.exit_code.ok_or(CliError::MissingExitCode)
}

/// Writes each non-fatal note to standard error.
fn report_warnings(warnings: &[String]) {
    let mut stderr = io::stderr();
    for warning in warnings {
        writeln!(stderr, "warning: {warning}").ok();
    }
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
pub(crate) async fn init_command(
    args: InitCommand,
    cancellation: &Cancellation,
) -> Result<i32, CliError> {
    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let init_config =
        InitConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    let sync_config =
        SyncConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;

    let backend = ScalewayBackend::new(scaleway_config.clone())
        .map_err(|err| CliError::Backend(err.to_string()))?;
    let syncer = Syncer::new(
        sync_config,
        StreamingCommandRunner::new(cancellation.scope()),
    )
    .map_err(|err| CliError::Sync(err.to_string()))?;

    let cwd = std::env::current_dir().map_err(|err| CliError::Config(err.to_string()))?;
    let cwd_path = Utf8PathBuf::from_path_buf(cwd)
        .map_err(|path| CliError::Config(path.display().to_string()))?;
    let project_name = cwd_path.file_name().unwrap_or("mriya");

    let request =
        InitRequest::from_config(&scaleway_config, &init_config, project_name, args.force)
            .map_err(|err| CliError::Config(err.to_string()))?;

    cli_state::warn_about_pending_journals();
    let ledger = cli_state::open_ledger(AllocationScope::new(
        request.volume.zone.clone(),
        request.volume.project_id.clone(),
    ))?;

    let orchestrator = InitOrchestrator::new(backend, syncer, ConfigStore::new());
    let outcome = orchestrator
        .execute(&request, &ledger, cancellation)
        .await?;

    report_warnings(&outcome.warnings);
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
pub(crate) fn build_backend_and_request(
    args: &RunCommand,
) -> Result<(ScalewayBackend, InstanceRequest), CliError> {
    let scaleway_config =
        ScalewayConfig::load_without_cli_args().map_err(|err| CliError::Config(err.to_string()))?;
    build_backend_and_request_with(&scaleway_config, args, &mockable::DefaultEnv)
}

/// Applies the command-line overrides, reading `~/` cloud-init paths through
/// `env`.
///
/// # Parameters
///
/// - `request`: the default request to modify in place.
/// - `args`: the parsed command line.
/// - `env`: environment used to expand a `~/` cloud-init file path.
///
/// # Errors
///
/// Returns [`CliError::InvalidOverride`] for a blank flag and
/// [`CliError::InvalidCloudInit`] for a conflicting or unreadable payload.
pub(crate) fn apply_instance_overrides_with(
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
/// For example, `--cloud-init "#cloud-config"` yields that payload, while
/// passing both `--cloud-init` and `--cloud-init-file` returns
/// [`CliError::InvalidCloudInit`] naming both flags.
/// Resolves the cloud-init flags, expanding a `~/` path through `env`.
///
/// # Parameters
///
/// - `args`: the parsed command line.
/// - `env`: environment used to expand a `~/` cloud-init file path.
///
/// # Returns
///
/// The user-data to send, or `None` when neither flag was supplied.
///
/// # Errors
///
/// Returns [`CliError::InvalidCloudInit`] for a conflicting, empty, or
/// unreadable payload.
pub(crate) fn resolve_cloud_init_for_run_with(
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
pub(crate) fn build_backend_and_request_with(
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
