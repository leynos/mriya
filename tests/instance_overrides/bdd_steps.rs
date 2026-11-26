//! BDD step definitions for instance overrides on `mriya run`.
//!
//! Every step drives the ordinary binary in an isolated environment, so the
//! remaining scenarios observe the diagnostics an invalid override produces
//! before provisioning starts.

use rstest_bdd_macros::{given, then, when};

use super::test_helpers::CliContext;
use crate::{
    cli_support::{check_run_failed_with, string_newtype},
    isolated_cli::CliOutput,
};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("assertion failed: {0}")]
    Assertion(String),
    #[error("failed to execute mriya command: {0}")]
    Execution(String),
}

string_newtype!(InstanceType);
string_newtype!(ImageLabel);
string_newtype!(ErrorSnippet);

/// Declares that the scenarios drive the binary a user receives.
///
/// There is no feature-gated variant to select, so the step only names the
/// subject; it keeps the feature file reading as a sentence.
#[given("the ordinary mriya binary")]
fn ordinary_binary(cli_context: CliContext) -> CliContext { cli_context }

/// Applies the scenario's instance type and image overrides, then captures the
/// output for the following behavioural assertions.
#[when("I run mriya with instance type \"{instance_type}\" and image \"{image}\"")]
fn run_with_overrides(
    mut cli_context: CliContext,
    instance_type: InstanceType,
    image: ImageLabel,
) -> Result<CliContext, StepError> {
    let run = cli_context
        .base_command()
        .map_err(|err| StepError::Execution(err.to_string()))?;
    let output = run
        .execute(&[
            "--instance-type",
            instance_type.as_ref(),
            "--image",
            image.as_ref(),
        ])
        .map_err(|err| StepError::Execution(err.to_string()))?;

    cli_context.output = Some(CliOutput::from_process_output(output));
    Ok(cli_context)
}

#[then("the run fails with error containing \"{snippet}\"")]
fn assert_run_fails_with_error(
    cli_context: &CliContext,
    snippet: ErrorSnippet,
) -> Result<(), StepError> {
    check_run_failed_with(cli_context.output.as_ref(), snippet.as_ref())
        .map_err(StepError::Assertion)
}
