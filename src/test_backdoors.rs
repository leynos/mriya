//! Environment-driven fake run modes used by CLI behavioural tests.
//!
//! Compiled only for unit tests and the `test-backdoors` feature, these
//! helpers let the binary short-circuit provisioning so tests can observe exit
//! codes, streamed output, and request construction without cloud access.
//!
//! Selection and execution are separate. [`select_fake_outcome`] reads the
//! injected environment and returns a value describing what was chosen, with
//! no side effects; [`run_fake_outcome`] is the command that performs it and
//! takes its output streams as arguments. That split is what lets a unit test
//! assert the precedence rules without capturing the test harness's own
//! standard streams, and assert the printed output without a process.

use std::io::Write;

use mockable::Env;
use mriya::{RunError, ScalewayBackendError};

use super::{CliError, RunCommand, build_backend_and_request};

/// A fake outcome chosen from the environment, before anything runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FakeOutcome {
    /// Simulate a run that prints its streamed output and reports an exit code.
    Run(FakeRunMode),
    /// Fail before provisioning, at the named lifecycle stage.
    Prefail(PrefailStage),
    /// Build a request and dump the fields the CLI behaviour suite asserts.
    DumpRequest,
}

/// The simulated run modes a test may select through `MRIYA_FAKE_RUN_MODE`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FakeRunMode {
    /// Print the streamed output and report success.
    Exit0,
    /// Print the streamed output and report exit status 7.
    Exit7,
    /// Print the streamed output, then report that no exit code was observed.
    MissingExit,
}

/// The lifecycle stages a test may fail early through `MRIYA_FAKE_RUN_PREFAIL`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrefailStage {
    /// Configuration loading.
    Config,
    /// Sync configuration loading.
    Sync,
    /// Backend construction.
    Backend,
    /// Provisioning.
    Run,
}

/// Returns the fake outcome the environment selects, if any.
///
/// Yields `None` unless `MRIYA_FAKE_RUN_ENABLE` is `1`, so callers fall
/// through to the real provisioning path. A run mode wins over a pre-failure
/// mode when both are set: both model a run the test asked for, and the run
/// mode is the more specific request.
pub(super) fn select_fake_outcome(env: &impl Env) -> Option<FakeOutcome> {
    if !enable_fake_modes(env) {
        return None;
    }
    fake_mode_from_env(env).or_else(|| prefail_from_env(env).map(FakeOutcome::Prefail))
}

/// Performs a selected fake outcome, writing to the streams it is given.
///
/// # Errors
///
/// Returns the injected `CliError` for a pre-failure stage, and
/// [`CliError::MissingExitCode`] for the `missing-exit` run mode. `dump-request`
/// can also fail while loading configuration or building the request.
pub(super) fn run_fake_outcome(
    args: &RunCommand,
    outcome: FakeOutcome,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<i32, CliError> {
    match outcome {
        FakeOutcome::Run(mode) => run_fake_mode(stdout, stderr, mode),
        FakeOutcome::Prefail(stage) => Err(stage.into_cli_error()),
        FakeOutcome::DumpRequest => dump_request(args, stdout),
    }
}

/// Reports whether the injected environment selects a fake run.
///
/// The mode variables are read only when this returns true, so an ordinary run
/// cannot be diverted by a stray `MRIYA_FAKE_*` variable in the environment.
fn enable_fake_modes(env: &impl Env) -> bool {
    env.string("MRIYA_FAKE_RUN_ENABLE")
        .is_some_and(|val| val == "1")
}

/// Reads the run mode `env` names in `MRIYA_FAKE_RUN_MODE`, if it names one.
fn fake_mode_from_env(env: &impl Env) -> Option<FakeOutcome> {
    match env.string("MRIYA_FAKE_RUN_MODE")?.as_str() {
        "exit-0" => Some(FakeOutcome::Run(FakeRunMode::Exit0)),
        "exit-7" => Some(FakeOutcome::Run(FakeRunMode::Exit7)),
        "missing-exit" => Some(FakeOutcome::Run(FakeRunMode::MissingExit)),
        "dump-request" => Some(FakeOutcome::DumpRequest),
        _ => None,
    }
}

/// Runs one simulated mode, streaming the fake output a real run would emit.
fn run_fake_mode(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    mode: FakeRunMode,
) -> Result<i32, CliError> {
    writeln!(stdout, "fake-stdout").ok();
    writeln!(stderr, "fake-stderr").ok();
    match mode {
        FakeRunMode::Exit0 => Ok(0),
        FakeRunMode::Exit7 => Ok(7),
        FakeRunMode::MissingExit => Err(CliError::MissingExitCode),
    }
}

/// Dumps the fields of the request the args describe.
///
/// This is the same request the real path would provision, so a behaviour
/// scenario can assert what the CLI made of its flags without cloud access.
fn dump_request(args: &RunCommand, stdout: &mut impl Write) -> Result<i32, CliError> {
    let (_backend, request) = build_backend_and_request(args)?;

    writeln!(stdout, "instance_type={}", request.instance_type).ok();
    writeln!(stdout, "image_label={}", request.image_label).ok();
    writeln!(
        stdout,
        "cloud_init_user_data_present={}",
        request.cloud_init_user_data.is_some()
    )
    .ok();
    writeln!(
        stdout,
        "cloud_init_user_data_size={}",
        request.cloud_init_user_data.as_deref().map_or(0, str::len)
    )
    .ok();
    Ok(0)
}

/// Reads the failure stage `env` names in `MRIYA_FAKE_RUN_PREFAIL`, if any.
fn prefail_from_env(env: &impl Env) -> Option<PrefailStage> {
    match env.string("MRIYA_FAKE_RUN_PREFAIL")?.as_str() {
        "config" => Some(PrefailStage::Config),
        "sync" => Some(PrefailStage::Sync),
        "backend" => Some(PrefailStage::Backend),
        "run" => Some(PrefailStage::Run),
        _ => None,
    }
}

impl PrefailStage {
    /// Builds the real `CliError` this stage would produce.
    ///
    /// Returning the production error type is what lets the CLI behaviour
    /// tests assert an early failure's exit code and message without needing
    /// cloud access or a sync target.
    fn into_cli_error(self) -> CliError {
        match self {
            Self::Config => CliError::Config(String::from("fake")),
            Self::Sync => CliError::Sync(String::from("fake")),
            Self::Backend => CliError::Backend(String::from("fake")),
            Self::Run => CliError::Run(RunError::Provision(ScalewayBackendError::Config(
                String::from("fake"),
            ))),
        }
    }
}
