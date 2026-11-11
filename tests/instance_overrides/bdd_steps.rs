//! BDD step definitions for instance overrides on `mriya run`.

use rstest_bdd_macros::{given, then, when};

use super::test_helpers::CliContext;
use crate::cli_support::{CliOutput, check_field_value, check_run_failed_with, string_newtype};

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

#[given("fake request dumping is enabled")]
fn fake_dumping_enabled(cli_context: CliContext) -> CliContext {
    cli_context
}

/// Runs the default `mriya run` path and retains its output for the following
/// behavioural assertions.
#[when("I run mriya without instance overrides")]
fn run_without_overrides(mut cli_context: CliContext) -> Result<CliContext, StepError> {
    let mut cmd = cli_context
        .base_command()
        .map_err(|err| StepError::Execution(err.to_string()))?;
    cmd.args(["run", "--", "echo", "ok"]);
    let output = cmd
        .output()
        .map_err(|err| StepError::Execution(err.to_string()))?;

    cli_context.output = Some(CliOutput::from_process_output(output));
    Ok(cli_context)
}

/// Applies the scenario's instance type and image overrides, then captures the
/// request-dump output for the following behavioural assertions.
#[when("I run mriya with instance type \"{instance_type}\" and image \"{image}\"")]
fn run_with_overrides(
    mut cli_context: CliContext,
    instance_type: InstanceType,
    image: ImageLabel,
) -> Result<CliContext, StepError> {
    let mut cmd = cli_context
        .base_command()
        .map_err(|err| StepError::Execution(err.to_string()))?;
    cmd.args([
        "run",
        "--instance-type",
        instance_type.as_ref(),
        "--image",
        image.as_ref(),
        "--",
        "echo",
        "ok",
    ]);
    let output = cmd
        .output()
        .map_err(|err| StepError::Execution(err.to_string()))?;

    cli_context.output = Some(CliOutput::from_process_output(output));
    Ok(cli_context)
}

fn assert_field_value(
    cli_context: &CliContext,
    key: &str,
    expected: &str,
) -> Result<(), StepError> {
    check_field_value(cli_context.output.as_ref(), key, expected).map_err(StepError::Assertion)
}

#[then("the run request uses instance type \"{instance_type}\"")]
fn assert_instance_type(
    cli_context: &CliContext,
    instance_type: InstanceType,
) -> Result<(), StepError> {
    assert_field_value(cli_context, "instance_type", instance_type.as_ref())
}

#[then("the run request uses image \"{image}\"")]
fn assert_image(cli_context: &CliContext, image: ImageLabel) -> Result<(), StepError> {
    assert_field_value(cli_context, "image_label", image.as_ref())
}

#[then("the run fails with error containing \"{snippet}\"")]
fn assert_run_fails_with_error(
    cli_context: &CliContext,
    snippet: ErrorSnippet,
) -> Result<(), StepError> {
    check_run_failed_with(cli_context.output.as_ref(), snippet.as_ref())
        .map_err(StepError::Assertion)
}
