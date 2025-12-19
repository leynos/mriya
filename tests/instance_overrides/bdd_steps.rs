//! BDD step definitions for instance overrides on `mriya run`.

use rstest_bdd_macros::{given, then, when};

use super::test_helpers::{CliContext, CliOutput};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("assertion failed: {0}")]
    Assertion(String),
    #[error("failed to execute mriya command: {0}")]
    Execution(String),
}

fn extract_value(stdout: &str, key: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .map(str::to_owned)
}

#[given("fake request dumping is enabled")]
fn fake_dumping_enabled(cli_context: CliContext) -> CliContext {
    cli_context
}

#[when("I run mriya without instance overrides")]
fn run_without_overrides(mut cli_context: CliContext) -> Result<CliContext, StepError> {
    let mut cmd = cli_context.base_command();
    cmd.args(["run", "--", "echo", "ok"]);
    let output = cmd
        .output()
        .map_err(|err| StepError::Execution(err.to_string()))?;

    cli_context.output = Some(CliOutput::from_process_output(output));
    Ok(cli_context)
}

#[when("I run mriya with instance type \"{instance_type}\" and image \"{image}\"")]
fn run_with_overrides(
    mut cli_context: CliContext,
    instance_type: String,
    image: String,
) -> Result<CliContext, StepError> {
    let mut cmd = cli_context.base_command();
    cmd.args([
        "run",
        "--instance-type",
        instance_type.as_str(),
        "--image",
        image.as_str(),
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

#[then("the run request uses instance type \"{instance_type}\"")]
fn assert_instance_type(cli_context: &CliContext, instance_type: String) -> Result<(), StepError> {
    let Some(output) = &cli_context.output else {
        return Err(StepError::Assertion(String::from("missing command output")));
    };
    let actual = extract_value(&output.stdout, "instance_type").ok_or_else(|| {
        StepError::Assertion(format!(
            "stdout missing instance_type key, got: {}",
            output.stdout
        ))
    })?;

    if actual != instance_type {
        return Err(StepError::Assertion(format!(
            "expected instance_type '{instance_type}', got '{actual}'"
        )));
    }
    Ok(())
}

#[then("the run request uses image \"{image}\"")]
fn assert_image(cli_context: &CliContext, image: String) -> Result<(), StepError> {
    let Some(output) = &cli_context.output else {
        return Err(StepError::Assertion(String::from("missing command output")));
    };
    let actual = extract_value(&output.stdout, "image_label").ok_or_else(|| {
        StepError::Assertion(format!(
            "stdout missing image_label key, got: {}",
            output.stdout
        ))
    })?;

    if actual != image {
        return Err(StepError::Assertion(format!(
            "expected image_label '{image}', got '{actual}'"
        )));
    }
    Ok(())
}

#[then("the run fails with error containing \"{snippet}\"")]
fn assert_run_fails_with_error(cli_context: &CliContext, snippet: String) -> Result<(), StepError> {
    let Some(output) = &cli_context.output else {
        return Err(StepError::Assertion(String::from("missing command output")));
    };
    if output.status_code == 0 {
        return Err(StepError::Assertion(String::from(
            "expected non-zero exit status",
        )));
    }
    if !output.stderr.contains(&snippet) {
        return Err(StepError::Assertion(format!(
            "expected stderr to contain '{snippet}', got: {}",
            output.stderr
        )));
    }
    Ok(())
}
