//! CLI entry point for Mriya.
//!
//! This binary provisions a short-lived Scaleway instance, synchronizes the
//! local workspace via `rsync`, executes a user-supplied command over SSH, and
//! cleans the instance up. The `run` subcommand preserves remote exit codes
//! locally and reports errors on stderr with meaningful exit statuses; `init`
//! prepares a cache volume, and `recover` finishes any allocation an
//! interrupted command left behind.

#[cfg(test)]
use std::{future::Future, pin::Pin};
use std::{
    io::{self, Write},
    process,
};

use clap::Parser;
use thiserror::Error;
#[cfg(test)]
use tokio::sync::Mutex;

mod cli;

mod cli_commands;

mod cli_state;

#[cfg(test)]
mod test_backdoors;

use cli::{Cli, InitCommand, RecoverCommand, RunCommand};
#[cfg(test)]
use cli_args::{parse_override, render_remote_command, validate_command_args};
#[cfg(test)]
pub(crate) use cli_commands::{apply_instance_overrides_with, build_backend_and_request_with};
use cli_commands::{init_command, run_command};
use mriya::{CancelHandle, Cancellation, InitError, RunError, ScalewayBackendError};
#[cfg(test)]
use mriya::{InstanceRequest, ScalewayConfig};

mod cli_args;

#[cfg(test)]
mod main_request_tests;

#[cfg(test)]
mod main_tests;

/// Failure modes of a CLI invocation, each rendered as one line on stderr.
///
/// Most variants lead to exit status 1, a remote command's own exit code is
/// passed through as the process status, and a cancellation exits 130 or 143 so
/// a caller can tell an interruption from a failure.
#[derive(Debug, Error)]
pub(crate) enum CliError {
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
    ///
    /// The error is boxed because it carries the primary failure, cleanup
    /// report, remote output, and notes; inlining it would enlarge every
    /// `Result` in the CLI's error path.
    #[error("remote run failed: {0}")]
    Run(Box<RunError<ScalewayBackendError>>),
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
    Init(Box<InitError<ScalewayBackendError>>),
}

impl From<RunError<ScalewayBackendError>> for CliError {
    fn from(error: RunError<ScalewayBackendError>) -> Self { Self::Run(Box::new(error)) }
}

impl From<InitError<ScalewayBackendError>> for CliError {
    fn from(error: InitError<ScalewayBackendError>) -> Self { Self::Init(Box::new(error)) }
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
    let (handle, cancellation) = CancelHandle::channel();
    if let Err(error) = handle.install_signal_handlers() {
        writeln!(
            io::stderr(),
            "warning: signal handlers are unavailable, so Ctrl+C cannot clean up: {error}"
        )
        .ok();
    }

    let result = match cli {
        Cli::Run(command) => exec_run(command, &cancellation).await,
        Cli::Init(command) => exec_init(command, &cancellation).await,
        Cli::Recover(command) => exec_recover(command, &cancellation).await,
    };
    result.unwrap_or_else(|err| {
        report_error(&err);
        exit_code_for(&err)
    })
}

/// Maps a reported failure to the process exit status.
///
/// Cancellation keeps the conventional signal status so a caller can tell an
/// interrupted run from a failed one; every other failure exits 1.
fn exit_code_for(err: &CliError) -> i32 {
    match err {
        CliError::Run(error) => run_exit_code(error.as_ref()),
        CliError::Init(error) => init_exit_code(error.as_ref()),
        _ => 1,
    }
}

/// Returns the signal status of a cancellation whose cleanup completed.
///
/// A cancellation whose cleanup also failed is a failure: the signal status
/// would tell a script the interruption was handled when resources may remain.
fn run_exit_code(error: &RunError<ScalewayBackendError>) -> i32 {
    match error {
        RunError::Cancelled(cancelled) if cancelled.cleanup.is_none() => {
            cancelled.signal.exit_code()
        }
        _ => 1,
    }
}

/// Returns the signal status of an interrupted `init` whose cleanup completed.
const fn init_exit_code(error: &InitError<ScalewayBackendError>) -> i32 {
    match error {
        InitError::Cancelled {
            signal, cleanup, ..
        } if cleanup.is_none() => signal.exit_code(),
        _ => 1,
    }
}

/// Runs the `run` subcommand.
///
/// In unit tests a hook installed in `RUN_COMMAND_HOOK` takes precedence, so
/// the dispatch can be tested without provisioning anything.
///
/// For example, with no hook installed the command is handed to
/// [`run_command`], whose exit status becomes the process status.
async fn exec_run(command: RunCommand, cancellation: &Cancellation) -> Result<i32, CliError> {
    #[cfg(test)]
    if let Some(hook) = RUN_COMMAND_HOOK.lock().await.as_ref() {
        return hook(command, cancellation.clone()).await;
    }

    run_command(command, cancellation).await
}

/// Runs the `init` subcommand.
///
/// For example, a successful init yields 0, while an invalid configuration
/// yields [`CliError::Config`] for the caller to report.
async fn exec_init(command: InitCommand, cancellation: &Cancellation) -> Result<i32, CliError> {
    init_command(command, cancellation).await
}

/// Runs the `recover` subcommand.
///
/// For example, a state directory holding one verified journal prints what was
/// removed and exits 0, while residual resources exit non-zero.
async fn exec_recover(
    _command: RecoverCommand,
    cancellation: &Cancellation,
) -> Result<i32, CliError> {
    cli_state::recover_command(cancellation).await
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
type RunHook = dyn Fn(RunCommand, Cancellation) -> Pin<Box<dyn Future<Output = Result<i32, CliError>> + Send>>
    + Send
    + Sync;

#[cfg(test)]
static RUN_COMMAND_HOOK: Mutex<Option<Box<RunHook>>> = Mutex::const_new(None);
// weave: run 'weave explain src/main.rs' for per-hunk detail, 'weave check' to verify your
// resolution
