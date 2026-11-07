//! Environment-driven fake run modes used by CLI behavioural tests.
//!
//! Compiled only for unit tests and the `test-backdoors` feature, these
//! helpers let the binary short-circuit provisioning so tests can observe exit
//! codes, streamed output, and request construction without cloud access.

use std::io::{self, Write};

use mockable::Env;
use mriya::{RunError, ScalewayBackendError};

use super::{CliError, RunCommand, build_backend_and_request};

/// Returns the outcome of a fake run when the environment selects one.
///
/// Yields `None` unless `MRIYA_FAKE_RUN_ENABLE` is `1` and either a fake run
/// mode or a pre-failure mode is selected, so callers fall through to the real
/// provisioning path.
pub(super) fn fake_outcome(args: &RunCommand, env: &impl Env) -> Option<Result<i32, CliError>> {
    if !enable_fake_modes(env) {
        return None;
    }
    fake_run_from_env(args, env).or_else(|| prefail_from_env(env).map(Err))
}

/// Reports whether the injected environment selects a fake run.
///
/// The mode variables are read only when this returns true, so an ordinary run
/// cannot be diverted by a stray `MRIYA_FAKE_*` variable in the environment.
fn enable_fake_modes(env: &impl Env) -> bool {
    env.string("MRIYA_FAKE_RUN_ENABLE")
        .is_some_and(|val| val == "1")
}

/// Runs the fake mode `env` names in `MRIYA_FAKE_RUN_MODE`, if it names one.
///
/// Returns `None` when no mode is selected, so the caller falls through to a
/// real provisioning run; `Some` short-circuits with the simulated exit status.
fn fake_run_from_env(args: &RunCommand, env: &impl Env) -> Option<Result<i32, CliError>> {
    let mode = env.string("MRIYA_FAKE_RUN_MODE")?;
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

/// Fails the run before provisioning, in the stage `env` names in
/// `MRIYA_FAKE_RUN_PREFAIL`.
///
/// Returning the real `CliError` for the named stage is what lets the CLI
/// behaviour tests assert the exit code and message of an early failure
/// without needing cloud access or a sync target.
fn prefail_from_env(env: &impl Env) -> Option<CliError> {
    let mode = env.string("MRIYA_FAKE_RUN_PREFAIL")?;
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
// weave: run 'weave explain src/test_backdoors.rs' for per-hunk detail, 'weave check' to verify your resolution
