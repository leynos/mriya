//! Environment-driven fake run modes used by CLI behavioural tests.
//!
//! Compiled only for unit tests and the `test-backdoors` feature, these
//! helpers let the binary short-circuit provisioning so tests can observe exit
//! codes, streamed output, and request construction without cloud access.

use std::env;
use std::io::{self, Write};

use mriya::{RunError, ScalewayBackendError};

use super::{CliError, RunCommand, build_backend_and_request};
/// Reports whether the CLI should consult the fake run modes.
///
/// The mode variables are read only when this returns true, so an ordinary run
/// cannot be diverted by a stray `MRIYA_FAKE_*` variable in the environment.
pub(super) fn enable_fake_modes() -> bool {
    env::var("MRIYA_FAKE_RUN_ENABLE").is_ok_and(|val| val == "1")
}

/// Runs the fake mode named by `MRIYA_FAKE_RUN_MODE`, if one is set.
///
/// Returns `None` when no mode is selected, so the caller falls through to a
/// real provisioning run; `Some` short-circuits with the simulated exit status.
pub(super) fn fake_run_from_env(args: &RunCommand) -> Option<Result<i32, CliError>> {
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

/// Fails the run before provisioning, in the stage named by
/// `MRIYA_FAKE_RUN_PREFAIL`.
///
/// Returning the real `CliError` for the named stage is what lets the CLI
/// behaviour tests assert the exit code and message of an early failure
/// without needing cloud access or a sync target.
pub(super) fn prefail_from_env() -> Option<CliError> {
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
