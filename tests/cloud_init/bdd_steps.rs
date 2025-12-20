//! BDD step definitions for cloud-init user-data CLI handling.

use std::fmt;
use std::fs;

use rstest_bdd_macros::{given, then, when};
use tempfile::TempDir;

use super::test_helpers::{CliContext, CliOutput};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("assertion failed: {0}")]
    Assertion(String),
    #[error("failed to execute mriya command: {0}")]
    Execution(String),
    #[error("failed to create temp file: {0}")]
    TempFile(String),
}

macro_rules! string_newtype {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        struct $name(String);

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.0.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_ref())
            }
        }

        impl std::str::FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self(value.to_owned()))
            }
        }
    };
}

string_newtype!(UserData);
string_newtype!(OutputKey);
string_newtype!(OutputValue);
string_newtype!(ErrorSnippet);
string_newtype!(FilePath);

fn extract_value(stdout: &str, key: &OutputKey) -> Option<String> {
    let key_value = key.as_ref();
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key_value}=")))
        .map(str::to_owned)
}

fn execute_run_command(
    mut cli_context: CliContext,
    additional_args: &[&str],
    tmp_dir: Option<TempDir>,
) -> Result<CliContext, StepError> {
    let mut cmd = cli_context.base_command();
    cmd.arg("run");
    cmd.args(additional_args);
    cmd.args(["--", "echo", "ok"]);
    let output = cmd
        .output()
        .map_err(|err| StepError::Execution(err.to_string()))?;

    if let Some(tmp_dir) = tmp_dir {
        cli_context.tmp_dir = Some(std::sync::Arc::new(tmp_dir));
    }

    cli_context.output = Some(CliOutput::from_process_output(output));
    Ok(cli_context)
}

#[given("fake request dumping is enabled for cloud-init")]
fn fake_dumping_enabled(cli_context: CliContext) -> CliContext {
    cli_context
}

#[when("I run mriya without cloud-init user-data")]
fn run_without_cloud_init(cli_context: CliContext) -> Result<CliContext, StepError> {
    execute_run_command(cli_context, &[], None)
}

#[when("I run mriya with inline cloud-init user-data \"{user_data}\"")]
fn run_with_inline_cloud_init(
    cli_context: CliContext,
    user_data: UserData,
) -> Result<CliContext, StepError> {
    execute_run_command(cli_context, &["--cloud-init", user_data.as_ref()], None)
}

#[when("I run mriya with cloud-init user-data file containing \"{content}\"")]
fn run_with_cloud_init_file(
    cli_context: CliContext,
    content: UserData,
) -> Result<CliContext, StepError> {
    let tmp_dir = TempDir::new().map_err(|err| StepError::TempFile(err.to_string()))?;
    let file_path = tmp_dir.path().join("user-data.txt");
    fs::write(&file_path, content.as_ref()).map_err(|err| StepError::TempFile(err.to_string()))?;

    let file_path_string = file_path
        .to_str()
        .ok_or_else(|| StepError::TempFile(String::from("non-utf8 file path")))?;

    execute_run_command(
        cli_context,
        &["--cloud-init-file", file_path_string],
        Some(tmp_dir),
    )
}

#[when("I run mriya with missing cloud-init file \"{path}\"")]
fn run_with_missing_cloud_init_file(
    cli_context: CliContext,
    path: FilePath,
) -> Result<CliContext, StepError> {
    execute_run_command(cli_context, &["--cloud-init-file", path.as_ref()], None)
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

    let output_key = OutputKey(key.to_owned());
    let actual = extract_value(&output.stdout, &output_key).ok_or_else(|| {
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

#[then("the run request has cloud-init user-data present \"{present}\"")]
fn assert_cloud_init_present(
    cli_context: &CliContext,
    present: OutputValue,
) -> Result<(), StepError> {
    assert_field_value(
        cli_context,
        "cloud_init_user_data_present",
        "cloud_init_user_data_present",
        present.as_ref(),
    )
}

#[then("the run request has cloud-init user-data size \"{size}\"")]
fn assert_cloud_init_size(cli_context: &CliContext, size: OutputValue) -> Result<(), StepError> {
    assert_field_value(
        cli_context,
        "cloud_init_user_data_size",
        "cloud_init_user_data_size",
        size.as_ref(),
    )
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
