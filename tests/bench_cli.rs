//! Contract tests for the `mriya-bench` binary's refusal paths.
//!
//! The binary can spend money, so its safety properties are tested as
//! behaviour rather than trusted: a paid run that is not enabled must fail
//! without reaching a provider, and the offline commands must work with no
//! credentials and no network.
//!
//! These tests run the binary with a cleared environment, so no credential in
//! the caller's shell can make a paid path look enabled.

use std::process::{Command, Output};

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use tempfile::TempDir;

/// Path to the built `mriya-bench` binary.
///
/// Cargo sets `CARGO_BIN_EXE_<name>` for integration tests in the same package,
/// so this is the artefact the same build produced rather than a separately
/// resolved path.
const BENCH_BINARY: &str = env!("CARGO_BIN_EXE_mriya-bench");

/// Runs `mriya-bench` with a cleared environment and the given arguments.
fn run(args: &[&str]) -> Output {
    Command::new(BENCH_BINARY)
        .args(args)
        .env_clear()
        .output()
        .expect("the benchmark binary should run")
}

/// Returns the process output's status code, treating a signal as 1.
fn status_code(output: &Output) -> i32 { output.status.code().unwrap_or(1) }

/// Returns a temporary file containing `text`.
fn temp_file(name: &str, text: &str) -> (TempDir, String) {
    let dir = TempDir::new().expect("a temporary directory");
    let temporary = Utf8Path::from_path(dir.path()).expect("a UTF-8 temporary path");
    let handle = Dir::open_ambient_dir(temporary, ambient_authority())
        .expect("the temporary directory should open");
    handle
        .write(name, text)
        .expect("the fixture file should write");
    let text_path = dir.path().join(name).to_string_lossy().into_owned();
    (dir, text_path)
}

/// Reads `path` through a capability handle on its parent directory.
fn read_file(path: &str) -> String {
    let parsed = Utf8Path::new(path);
    let parent = parsed
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .unwrap_or_else(|| Utf8Path::new("."));
    let name = parsed
        .file_name()
        .expect("the path should name a file")
        .to_owned();
    let handle = Dir::open_ambient_dir(parent, ambient_authority())
        .expect("the parent directory should open");
    handle.read_to_string(name).expect("the file should read")
}

/// The path of the repository's example campaign plan.
fn example_campaign_path() -> String {
    format!(
        "{}/benchmarks/campaign.example.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// The path of the repository's example price sheet.
fn example_price_sheet_path() -> String {
    format!(
        "{}/benchmarks/price-sheet.example.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// The path of the repository's valid campaign fixture.
fn valid_records_path() -> String {
    format!(
        "{}/src/bench/fixtures/valid-campaign.jsonl",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn validate_accepts_a_valid_campaign_without_credentials() {
    let output = run(&["validate", &valid_records_path()]);
    assert_eq!(status_code(&output), 0, "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("example-2026-11-26"), "{stdout}");
    assert!(stdout.contains("1 runs"), "{stdout}");
}

#[test]
fn validate_reports_every_defect_it_finds() {
    let fixture = format!(
        "{}/src/bench/fixtures/defective-campaign.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let output = run(&["validate", &fixture]);
    assert_eq!(status_code(&output), 1, "a defective campaign must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("campaign header repeated"), "{stderr}");
    assert!(stderr.contains("malformed record"), "{stderr}");
    assert!(stderr.lines().count() >= 10, "{stderr}");
}

#[test]
fn validate_reports_a_file_with_no_runs() {
    let fixture = format!(
        "{}/src/bench/fixtures/header-only.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let output = run(&["validate", &fixture]);
    assert_eq!(status_code(&output), 1);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no run records"),
        "{output:?}"
    );
}

#[test]
fn validate_reports_a_missing_file_without_panicking() {
    let output = run(&["validate", "/nonexistent/campaign.jsonl"]);
    assert_eq!(status_code(&output), 1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot read"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn report_prices_a_campaign_offline() {
    let output = run(&[
        "report",
        &valid_records_path(),
        "--price-sheet",
        &example_price_sheet_path(),
    ]);
    assert_eq!(status_code(&output), 0, "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("campaign example-2026-11-26"), "{stdout}");
    assert!(
        stdout.contains("marginal cost per completed gate"),
        "{stdout}"
    );
}

#[test]
fn report_refuses_a_defective_campaign() {
    let defective = format!(
        "{}/src/bench/fixtures/defective-campaign.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let output = run(&[
        "report",
        &defective,
        "--price-sheet",
        &example_price_sheet_path(),
    ]);
    assert_eq!(status_code(&output), 1);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("campaign header repeated"),
        "{output:?}"
    );
}

#[test]
fn report_refuses_an_unreadable_price_sheet() {
    let (_dir, sheet) = temp_file("sheet.toml", "[sheet]\n");
    let output = run(&["report", &valid_records_path(), "--price-sheet", &sheet]);
    assert_eq!(status_code(&output), 1);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("currency"),
        "{output:?}"
    );
}

#[test]
fn a_paid_run_refuses_to_start_without_the_opt_in() {
    let (_dir, out) = temp_file("results.jsonl", "");
    let output = run(&["run", "--campaign", &example_campaign_path(), "--out", &out]);
    assert_eq!(
        status_code(&output),
        1,
        "a campaign without the opt-in must fail: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MRIYA_BENCH_ENABLE_PAID_RUNS"),
        "the refusal names the variable: {stderr}"
    );
    assert!(
        stderr.contains("set MRIYA_BENCH_ENABLE_PAID_RUNS=1"),
        "the refusal says what to set: {stderr}"
    );
    // Nothing may have been appended, because no allocation could have started.
    let written = read_file(&out);
    assert!(
        written.is_empty(),
        "a refused campaign must not write a record: {written}"
    );
}

#[test]
fn a_paid_run_refuses_a_campaign_that_declares_no_limits() {
    let (_dir, campaign) = temp_file(
        "campaign.toml",
        "[campaign]\nid = \"c\"\n[campaign.limits]\n",
    );
    let (_out_dir, out) = temp_file("results.jsonl", "");
    let output = run(&["run", "--campaign", &campaign, "--out", &out]);
    assert_eq!(status_code(&output), 1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("campaign is not readable"), "{stderr}");
}

#[test]
fn the_help_text_names_all_three_subcommands() {
    let output = run(&["--help"]);
    assert_eq!(status_code(&output), 0, "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in ["validate", "report", "run"] {
        assert!(stdout.contains(command), "{command} missing from {stdout}");
    }
}
