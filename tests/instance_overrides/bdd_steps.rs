//! BDD step definitions for instance overrides on `mriya run`.

use std::fmt;

use rstest_bdd_macros::{given, then, when};

use super::test_helpers::{CliContext, CliOutput};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("assertion failed: {0}")]
    Assertion(String),
    #[error("failed to execute mriya command: {0}")]
    Execution(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InstanceType(String);

impl From<String> for InstanceType {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for InstanceType {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl fmt::Display for InstanceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for InstanceType {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(value.to_owned()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ImageLabel(String);

impl From<String> for ImageLabel {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for ImageLabel {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl fmt::Display for ImageLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for ImageLabel {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(value.to_owned()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ErrorSnippet(String);

impl From<String> for ErrorSnippet {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for ErrorSnippet {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl fmt::Display for ErrorSnippet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for ErrorSnippet {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(value.to_owned()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OutputKey(String);

impl From<String> for OutputKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for OutputKey {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl fmt::Display for OutputKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

fn extract_value(stdout: &str, key: OutputKey) -> Option<String> {
    let OutputKey(key_value) = key;
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key_value}=")))
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
    instance_type: InstanceType,
    image: ImageLabel,
) -> Result<CliContext, StepError> {
    let mut cmd = cli_context.base_command();
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
    field_name: &str,
    key: &str,
    expected: &str,
) -> Result<(), StepError> {
    let Some(output) = &cli_context.output else {
        return Err(StepError::Assertion(String::from("missing command output")));
    };

    let actual = extract_value(&output.stdout, OutputKey(key.to_owned())).ok_or_else(|| {
        StepError::Assertion(format!(
            "stdout missing {field_name} key, got: {}",
            output.stdout
        ))
    })?;

    if actual != expected {
        return Err(StepError::Assertion(format!(
            "expected {field_name} '{expected}', got '{actual}'"
        )));
    }

    Ok(())
}

#[then("the run request uses instance type \"{instance_type}\"")]
fn assert_instance_type(
    cli_context: &CliContext,
    instance_type: InstanceType,
) -> Result<(), StepError> {
    assert_field_value(
        cli_context,
        "instance_type",
        "instance_type",
        instance_type.as_ref(),
    )
}

#[then("the run request uses image \"{image}\"")]
fn assert_image(cli_context: &CliContext, image: ImageLabel) -> Result<(), StepError> {
    assert_field_value(cli_context, "image_label", "image_label", image.as_ref())
}

#[then("the run fails with error containing \"{snippet}\"")]
fn assert_run_fails_with_error(
    cli_context: &CliContext,
    snippet: ErrorSnippet,
) -> Result<(), StepError> {
    let Some(output) = &cli_context.output else {
        return Err(StepError::Assertion(String::from("missing command output")));
    };
    if output.status_code == 0 {
        return Err(StepError::Assertion(String::from(
            "expected non-zero exit status",
        )));
    }
    if !output.stderr.contains(snippet.as_ref()) {
        return Err(StepError::Assertion(format!(
            "expected stderr to contain '{snippet}', got: {}",
            output.stderr
        )));
    }
    Ok(())
}
