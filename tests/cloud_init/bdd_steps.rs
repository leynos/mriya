//! BDD step definitions for cloud-init user-data CLI handling.

use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs_utf8::Dir};
use rstest_bdd_macros::{given, then, when};
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use tempfile::TempDir;

use super::test_helpers::CliContext;
use crate::cli_support::{CliOutput, check_field_value, check_run_failed_with, string_newtype};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("assertion failed: {0}")]
    Assertion(String),
    #[error("failed to execute mriya command: {0}")]
    Execution(String),
    #[error("failed to create temp file: {0}")]
    TempFile(String),
}

string_newtype!(UserData);
string_newtype!(OutputValue);
string_newtype!(ErrorSnippet);
string_newtype!(FilePath);

/// Runs the compiled CLI in fake request-dump mode and stores its output in
/// the returned context.
///
/// `additional_args` are placed before the fixed `-- echo ok` payload. When a
/// cloud-init file is supplied, `tmp_dir` is retained in the returned context
/// so the file remains available until that context is dropped. Command
/// construction and process-spawn failures become `StepError::Execution`.
///
/// # Example
///
/// A file-backed step passes `--cloud-init-file` in `additional_args` and the
/// directory that owns the file; the resulting context contains the captured
/// status, stdout, and stderr.
fn execute_run_command(
    cli_context: CliContext,
    additional_args: &[&str],
    tmp_dir: Option<TempDir>,
) -> Result<CliContext, StepError> {
    run_with_home(cli_context, additional_args, tmp_dir, None)
}

/// Runs the CLI with an optional `HOME` override for one scenario.
///
/// The override matters only to tilde expansion, which the child binary reads
/// through its own environment; every other scenario passes `None` and
/// inherits the test process's own. Command construction and process-spawn
/// failures become `StepError::Execution`.
fn run_with_home(
    cli_context: CliContext,
    additional_args: &[&str],
    tmp_dir: Option<TempDir>,
    home: Option<&str>,
) -> Result<CliContext, StepError> {
    run_with_home_os(
        cli_context,
        additional_args,
        tmp_dir,
        home.map(std::ffi::OsString::from),
    )
}

/// Runs the CLI with an optional `HOME` override, already in OS-string form.
///
/// The `OsString` form exists for a value that is deliberately not UTF-8: a
/// `&str` could not express the case the expansion must report. Command
/// construction and process-spawn failures become `StepError::Execution`.
fn run_with_home_os(
    mut cli_context: CliContext,
    additional_args: &[&str],
    tmp_dir: Option<TempDir>,
    home: Option<std::ffi::OsString>,
) -> Result<CliContext, StepError> {
    let mut cmd = cli_context
        .base_command()
        .map_err(|err| StepError::Execution(err.to_string()))?;
    if let Some(directory) = home {
        cmd.env("HOME", directory);
    }
    cmd.arg("run");
    cmd.args(additional_args);
    cmd.args(["--", "echo", "ok"]);
    let output = cmd
        .output()
        .map_err(|err| StepError::Execution(err.to_string()))?;

    if let Some(temp_dir) = tmp_dir {
        cli_context.tmp_dir = Some(std::sync::Arc::new(temp_dir));
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
    let tmp_root = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).map_err(|path| {
        StepError::TempFile(format!("non-utf8 temp dir path: {}", path.display()))
    })?;
    Dir::open_ambient_dir(&tmp_root, ambient_authority())
        .map_err(|err| StepError::TempFile(err.to_string()))?
        .write("user-data.txt", content.as_ref())
        .map_err(|err| StepError::TempFile(err.to_string()))?;

    let file_path_string = file_path
        .to_str()
        .ok_or_else(|| StepError::TempFile(String::from("non-utf8 file path")))?;

    execute_run_command(
        cli_context,
        &["--cloud-init-file", file_path_string],
        Some(tmp_dir),
    )
}

#[when("I run mriya with cloud-init file \"{path}\" relative to the temporary home")]
fn run_with_cloud_init_file_under_home(
    cli_context: CliContext,
    path: FilePath,
) -> Result<CliContext, StepError> {
    let tmp_dir = TempDir::new().map_err(|err| StepError::TempFile(err.to_string()))?;
    let home = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf())
        .map_err(|raw| StepError::TempFile(format!("non-utf8 temp dir path: {}", raw.display())))?;
    // The feature file names the file; its content is what the assertion
    // measures, so the step writes a fixed payload and keeps the stated size
    // from drifting if the scenario is reused for another path.
    let content = "file-user-data";
    Dir::open_ambient_dir(&home, ambient_authority())
        .map_err(|err| StepError::TempFile(err.to_string()))?
        .write(path.as_ref(), content)
        .map_err(|err| StepError::TempFile(err.to_string()))?;
    let tilde_path = format!("~/{path}");
    let home_text = home.as_str();
    run_with_home(
        cli_context,
        &["--cloud-init-file", &tilde_path],
        Some(tmp_dir),
        Some(home_text),
    )
}

/// Steps the non-UTF-8 `HOME` scenario, which only Unix can express.
///
/// The binding needs a step per phrase either way, so the Windows build keeps
/// the phrase and fails the assertion: a scenario that quietly stopped
/// exercising the undecodable case would be worse than a named gap.
#[when("I run mriya with a tilde cloud-init file and a non-UTF-8 HOME")]
#[cfg(unix)]
fn run_with_tilde_cloud_init_and_undecodable_home(
    cli_context: CliContext,
) -> Result<CliContext, StepError> {
    // The undecodable value rides on `OsStr` bytes, so the child — not the test
    // process — carries it. That is the same path a user's environment takes.
    let undecodable_home = std::ffi::OsString::from_vec(b"/home/\xff".to_vec());
    run_with_home_os(
        cli_context,
        &["--cloud-init-file", "~/user-data.yml"],
        None,
        Some(undecodable_home),
    )
}

#[when("I run mriya with a tilde cloud-init file and a non-UTF-8 HOME")]
#[cfg(not(unix))]
fn run_with_tilde_cloud_init_and_undecodable_home_unix_only(
    _cli_context: CliContext,
) -> Result<CliContext, StepError> {
    Err(StepError::Assertion(String::from(
        "a non-UTF-8 HOME needs a Unix-style OsString and cannot be expressed on this platform",
    )))
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
    key: &str,
    expected: &str,
) -> Result<(), StepError> {
    check_field_value(cli_context.output.as_ref(), key, expected).map_err(StepError::Assertion)
}

macro_rules! cloud_init_field_assertion {
    ($fn_name:ident, $step_pattern:literal, $field:literal, $param:ident) => {
        #[then($step_pattern)]
        fn $fn_name(cli_context: &CliContext, $param: OutputValue) -> Result<(), StepError> {
            assert_field_value(cli_context, $field, $param.as_ref())
        }
    };
}

cloud_init_field_assertion!(
    assert_cloud_init_present,
    "the run request has cloud-init user-data present \"{present}\"",
    "cloud_init_user_data_present",
    present
);

cloud_init_field_assertion!(
    assert_cloud_init_size,
    "the run request has cloud-init user-data size \"{size}\"",
    "cloud_init_user_data_size",
    size
);

#[then("the run fails with error containing \"{snippet}\"")]
fn assert_run_fails_with_error(
    cli_context: &CliContext,
    snippet: ErrorSnippet,
) -> Result<(), StepError> {
    check_run_failed_with(cli_context.output.as_ref(), snippet.as_ref())
        .map_err(StepError::Assertion)
}
