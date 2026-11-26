//! BDD step definitions for cloud-init user-data CLI handling.
//!
//! Every step drives the ordinary binary in an isolated environment. The
//! remaining scenarios all fail before provisioning, so their evidence is the
//! diagnostic the binary prints, not a request dump: the dump only existed
//! behind the retired test-only feature.

use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs_utf8::Dir};
use rstest_bdd_macros::{given, then, when};
use tempfile::TempDir;

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
    #[error("failed to create temp file: {0}")]
    TempFile(String),
}

string_newtype!(UserData);
string_newtype!(ErrorSnippet);
string_newtype!(FilePath);

/// Declares that the scenarios drive the binary a user receives.
///
/// There is no feature-gated variant to select, so the step only names the
/// subject; it keeps the feature file reading as a sentence.
#[given("the ordinary mriya binary")]
fn ordinary_binary(cli_context: CliContext) -> CliContext { cli_context }

/// Runs the isolated `mriya run` with `additional_args` and captures its
/// output.
///
/// Command construction and process-spawn failures become
/// `StepError::Execution`.
fn execute_run_command(
    cli_context: CliContext,
    additional_args: &[&str],
) -> Result<CliContext, StepError> {
    run_with_home(cli_context, additional_args, None)
}

/// Runs the isolated CLI with an optional `HOME` override for one scenario.
///
/// The override matters only to tilde expansion, which the child binary reads
/// through its own environment; every other scenario passes `None` and
/// inherits the isolated temporary home.
fn run_with_home(
    cli_context: CliContext,
    additional_args: &[&str],
    home: Option<&str>,
) -> Result<CliContext, StepError> {
    run_with_home_os(cli_context, additional_args, home.map(OsStr::new))
}

/// Runs the isolated CLI with an optional `HOME` override, already in OS-string
/// form.
///
/// The `OsStr` form exists for a value that is deliberately not UTF-8: a
/// `&str` could not express the case the expansion must report.
fn run_with_home_os(
    mut cli_context: CliContext,
    additional_args: &[&str],
    home: Option<&OsStr>,
) -> Result<CliContext, StepError> {
    let run = cli_context
        .base_command(home)
        .map_err(|err| StepError::Execution(err.to_string()))?;
    let output = run
        .execute(additional_args)
        .map_err(|err| StepError::Execution(err.to_string()))?;

    cli_context.output = Some(CliOutput::from_process_output(output));
    Ok(cli_context)
}

#[when("I run mriya with inline cloud-init user-data \"{user_data}\"")]
fn run_with_inline_cloud_init(
    cli_context: CliContext,
    user_data: UserData,
) -> Result<CliContext, StepError> {
    execute_run_command(cli_context, &["--cloud-init", user_data.as_ref()])
}

/// Runs with a `--cloud-init-file` named relative to a temporary home.
///
/// The file is written before the child starts and the temporary directory is
/// dropped after it exits, so the fixture exists for exactly as long as the
/// binary can read it. What the scenario can still prove is that the path was
/// expanded and read: a path that failed to expand would be reported as an
/// unreadable file, not as the control-character rejection every isolated run
/// ends with.
#[when("I run mriya with cloud-init file \"{path}\" relative to the temporary home")]
fn run_with_cloud_init_file_under_home(
    cli_context: CliContext,
    path: FilePath,
) -> Result<CliContext, StepError> {
    let tmp_dir = TempDir::new().map_err(|err| StepError::TempFile(err.to_string()))?;
    let home = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf())
        .map_err(|raw| StepError::TempFile(format!("non-utf8 temp dir path: {}", raw.display())))?;
    // The feature file names the file; its content is what the assertion
    // measures, so the step writes a fixed payload.
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
        Some(home_text),
    )
}

/// Steps the non-UTF-8 `HOME` scenario, which is built only on Unix.
///
/// The scenario carries `@unix` in the feature file and is filtered out at
/// registration on other platforms, so neither this step nor the scenario
/// exists there. A Windows build therefore has no binding for the phrase,
/// which is visible in the registration rather than as a scenario that runs
/// and fails.
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
        Some(undecodable_home.as_os_str()),
    )
}

#[when("I run mriya with missing cloud-init file \"{path}\"")]
fn run_with_missing_cloud_init_file(
    cli_context: CliContext,
    path: FilePath,
) -> Result<CliContext, StepError> {
    execute_run_command(cli_context, &["--cloud-init-file", path.as_ref()])
}

#[then("the run fails with error containing \"{snippet}\"")]
fn assert_run_fails_with_error(
    cli_context: &CliContext,
    snippet: ErrorSnippet,
) -> Result<(), StepError> {
    check_run_failed_with(cli_context.output.as_ref(), snippet.as_ref())
        .map_err(StepError::Assertion)
}
