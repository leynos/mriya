//! The Whitaker half of the lint gate: that `make lint` runs the docstring gate, rustdoc and Clippy
//! before the Whitaker suite even under `make -j`, and that a failing Whitaker fails the route. The
//! Make routes run for real against a sandbox `PATH` of fakes, so a soft-failed or missing Whitaker
//! line shows up as a passing route or an absent log entry. The Makefile and CI shapes those routes
//! obey are read by the sibling `whitaker_recipes` module.

use rstest::rstest;

use super::{
    build_tools_scripts::{Toolset, cargo_calls, sandbox_with},
    sandbox::Sandbox,
};

/// The exit status the failing fake Whitaker returns, distinct from Make's own `2`.
const WHITAKER_FAILURE: i32 = 7;

/// A Make route that reaches the Whitaker gate.
#[derive(Clone, Copy)]
enum Route {
    /// The composite `lint` target: Clippy first, Whitaker last.
    Composite,
    /// The standalone Whitaker half.
    WhitakerOnly,
}

impl Route {
    /// The target name to pass to `make`.
    fn name(self) -> &'static str {
        match self {
            Self::Composite => "lint",
            Self::WhitakerOnly => "lint-whitaker",
        }
    }
}

/// One tool invocation the fakes record, in the order they ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    /// The docstring gate.
    Interrogate,
    /// `cargo doc`.
    Doc,
    /// `cargo clippy`.
    Clippy,
    /// The Whitaker Dylint suite.
    Whitaker,
    /// A call the contract does not expect; it fails an order comparison.
    Unexpected,
}

impl Call {
    /// Classifies a logged call by its first word.
    ///
    /// A word the contract does not expect becomes `Unexpected`, so an extra tool call still fails
    /// an order comparison instead of being filtered away.
    fn of_line(line: &str) -> Self {
        match line.split_whitespace().next() {
            Some("interrogate") => Self::Interrogate,
            Some("doc") => Self::Doc,
            Some("clippy") => Self::Clippy,
            Some("whitaker") => Self::Whitaker,
            _ => Self::Unexpected,
        }
    }
}

/// A sandbox with a slow docstring gate, so a gate that ran in parallel would log after Whitaker.
fn lint_sandbox(whitaker_status: i32) -> Result<Sandbox, String> {
    let sandbox = sandbox_with(Toolset::COMPLETE)?;
    sandbox.link_tools(&["sleep"])?;
    sandbox.write_tool(
        "fake-interrogate",
        "sleep 0.4; echo interrogate >> \"$CARGO_LOG\"",
    )?;
    sandbox.write_tool(
        "fake-whitaker",
        format!(
            "echo \"whitaker $*\" >> \"$CARGO_LOG\"; echo \"RUSTFLAGS=$RUSTFLAGS \
             encoded=${{CARGO_ENCODED_RUSTFLAGS-unset}}\" > \"$CARGO_LOG.env\"; exit \
             {whitaker_status}"
        ),
    )?;
    Ok(sandbox)
}

/// Runs `make -B -j<jobs> <route>` with the fakes substituted for every external tool.
fn run_lint_route(
    sandbox: &Sandbox,
    jobs: u8,
    route: Route,
) -> Result<std::process::Output, String> {
    sandbox
        .command("make")?
        .args([
            "-B",
            &format!("-j{jobs}"),
            "CARGO=probe-cargo",
            "INTERROGATE=fake-interrogate",
            "WHITAKER=fake-whitaker",
            route.name(),
        ])
        .output()
        .map_err(|error| format!("running make {}: {error}", route.name()))
}

/// The logged tool calls, in order: `interrogate`, `doc`, `clippy`, `whitaker`.
fn call_order(sandbox: &Sandbox) -> Result<Vec<Call>, String> {
    Ok(cargo_calls(sandbox)?
        .iter()
        .map(|line| Call::of_line(line))
        .collect())
}

/// Scenario: `make lint` run for real at several job counts.
///
/// Invariant: the docstring gate, rustdoc and Clippy run in that order and Whitaker runs last,
/// whatever `-j` asks for, because the composite target is built serially.
#[rstest]
#[case::serial(1)]
#[case::parallel(4)]
#[case::wide(16)]
fn lint_runs_whitaker_last_whatever_the_job_count(#[case] jobs: u8) -> Result<(), String> {
    let sandbox = lint_sandbox(0)?;
    let output = run_lint_route(&sandbox, jobs, Route::Composite)?;
    if !output.status.success() {
        return Err(format!(
            "`make -j{jobs} lint` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let order = call_order(&sandbox)?;
    if order == [Call::Interrogate, Call::Doc, Call::Clippy, Call::Whitaker] {
        Ok(())
    } else {
        Err(format!("`make -j{jobs} lint` ran {order:?}"))
    }
}

/// Scenario: a Whitaker that exits non-zero, under each route that reaches it.
///
/// Invariant: Make fails with a non-zero status and names the recipe's own status, so the gate
/// binds; the standalone Whitaker route runs nothing before it.
#[rstest]
#[case::composite(Route::Composite, &[Call::Interrogate, Call::Doc, Call::Clippy, Call::Whitaker])]
#[case::standalone(Route::WhitakerOnly, &[Call::Whitaker])]
fn a_failing_whitaker_fails_the_route(
    #[case] route: Route,
    #[case] expected_order: &[Call],
) -> Result<(), String> {
    let sandbox = lint_sandbox(WHITAKER_FAILURE)?;
    let output = run_lint_route(&sandbox, 4, route)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        return Err(format!(
            "`make {}` passed past a failing Whitaker",
            route.name()
        ));
    }
    if !stderr.contains(&format!("Error {WHITAKER_FAILURE}")) {
        return Err(format!(
            "`make {}` hid Whitaker's status:\n{stderr}",
            route.name()
        ));
    }
    let order = call_order(&sandbox)?;
    if order == expected_order {
        Ok(())
    } else {
        Err(format!("`make {}` ran {order:?}", route.name()))
    }
}

/// Scenario: `make lint-whitaker` run with development flags in the caller's environment.
///
/// Invariant: Whitaker sees the warning policy alone, and `CARGO_ENCODED_RUSTFLAGS`, which Cargo
/// reads in preference to `RUSTFLAGS`, is gone, so the development flags never reach its toolchain.
#[test]
fn whitaker_runs_with_the_warning_policy_alone() -> Result<(), String> {
    let sandbox = lint_sandbox(0)?;
    let output = sandbox
        .command("make")?
        .env("RUSTFLAGS", "-Zthreads=8")
        .env("CARGO_ENCODED_RUSTFLAGS", "-Zthreads=8")
        .args(["-B", "WHITAKER=fake-whitaker", "lint-whitaker"])
        .output()
        .map_err(|error| format!("running make lint-whitaker: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let seen = sandbox
        .read("cargo.log.env")?
        .ok_or("Whitaker recorded no environment")?;
    if seen.trim() == "RUSTFLAGS=-D warnings encoded=unset" {
        Ok(())
    } else {
        Err(format!("Whitaker saw `{}`", seen.trim()))
    }
}

/// Scenario: a failing Clippy step before Whitaker.
///
/// Invariant: `doc` succeeds, Clippy is the step that fails, and the composite
/// stops there without starting Whitaker.
#[test]
fn a_failing_clippy_stops_lint_before_whitaker() -> Result<(), String> {
    let sandbox = lint_sandbox(0)?;
    // Fail only Clippy. A fake that fails every Cargo call would fail at `doc`
    // and never exercise the Clippy step this test names.
    sandbox.write_tool(
        "probe-cargo",
        "echo \"$*\" >> \"$CARGO_LOG\"; case \"$1\" in clippy) exit 1;; *) exit 0;; esac",
    )?;
    let output = run_lint_route(&sandbox, 4, Route::Composite)?;
    if output.status.success() {
        return Err("`make lint` passed with a failing Clippy".to_owned());
    }
    let order = call_order(&sandbox)?;
    if order == [Call::Interrogate, Call::Doc, Call::Clippy] {
        Ok(())
    } else {
        Err(format!("failing Clippy route ran {order:?}"))
    }
}
