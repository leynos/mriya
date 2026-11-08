//! The Whitaker half of the lint gate: that `make lint` runs the docstring gate, rustdoc and Clippy
//! before the Whitaker suite even under `make -j`, and that a failing Whitaker fails the route and
//! the CI step around it. The Make routes run for real against a sandbox `PATH` of fakes, so a
//! soft-failed or missing Whitaker line shows up as a passing route or an absent log entry.

use rstest::rstest;

use super::{
    build_tools_scripts::{Toolset, cargo_calls, sandbox_with},
    sandbox::Sandbox,
};

const MAKEFILE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Makefile"));
const CI_WORKFLOW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.github/workflows/ci.yml"
));

/// The exit status the failing fake Whitaker returns, distinct from Make's own `2`.
const WHITAKER_FAILURE: i32 = 7;

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
        &format!(
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
    route: &str,
) -> Result<std::process::Output, String> {
    sandbox
        .command("make")?
        .args([
            "-B",
            &format!("-j{jobs}"),
            "CARGO=probe-cargo",
            "INTERROGATE=fake-interrogate",
            "WHITAKER=fake-whitaker",
            route,
        ])
        .output()
        .map_err(|error| format!("running make {route}: {error}"))
}

/// The first word of each logged tool call: `interrogate`, `doc`, `clippy`, `whitaker`.
fn call_order(sandbox: &Sandbox) -> Result<Vec<String>, String> {
    Ok(cargo_calls(sandbox)?
        .iter()
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
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
    let output = run_lint_route(&sandbox, jobs, "lint")?;
    if !output.status.success() {
        return Err(format!(
            "`make -j{jobs} lint` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let order = call_order(&sandbox)?;
    if order == ["interrogate", "doc", "clippy", "whitaker"] {
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
#[case::composite("lint", &["interrogate", "doc", "clippy", "whitaker"])]
#[case::standalone("lint-whitaker", &["whitaker"])]
fn a_failing_whitaker_fails_the_route(
    #[case] route: &str,
    #[case] expected_order: &[&str],
) -> Result<(), String> {
    let sandbox = lint_sandbox(WHITAKER_FAILURE)?;
    let output = run_lint_route(&sandbox, 4, route)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        return Err(format!("`make {route}` passed past a failing Whitaker"));
    }
    if !stderr.contains(&format!("Error {WHITAKER_FAILURE}")) {
        return Err(format!("`make {route}` hid Whitaker's status:\n{stderr}"));
    }
    let order = call_order(&sandbox)?;
    if order == expected_order {
        Ok(())
    } else {
        Err(format!("`make {route}` ran {order:?}"))
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
/// Invariant: the composite stops there and never starts Whitaker.
#[test]
fn a_failing_clippy_stops_lint_before_whitaker() -> Result<(), String> {
    let sandbox = lint_sandbox(0)?;
    sandbox.write_tool("probe-cargo", "echo \"$*\" >> \"$CARGO_LOG\"; exit 1")?;
    let output = run_lint_route(&sandbox, 4, "lint")?;
    if output.status.success() {
        return Err("`make lint` passed with a failing Cargo".to_owned());
    }
    let order = call_order(&sandbox)?;
    if order.contains(&"whitaker".to_owned()) {
        Err(format!("Whitaker ran after a failing step: {order:?}"))
    } else {
        Ok(())
    }
}

/// Returns the recipe lines of a Make rule, tab stripped, or `None` when the rule is absent.
fn recipe_of<'a>(makefile: &'a str, target: &str) -> Option<Vec<&'a str>> {
    let header = format!("{target}:");
    let mut lines = makefile
        .lines()
        .skip_while(|line| !line.starts_with(&header))
        .skip(1);
    let recipe: Vec<&str> = lines
        .by_ref()
        .take_while(|line| line.starts_with('\t'))
        .map(|line| line.trim_start_matches('\t'))
        .collect();
    (!recipe.is_empty()).then_some(recipe)
}

/// Returns why a gate recipe could be soft-failed, or an empty list when it cannot.
fn masking_problems(recipe: &[&str], gate: &str) -> Vec<String> {
    const MASKS: &[&str] = &["||", "|", ";", "command -v", "which ", "true", "exit 0"];
    let mut problems = Vec::new();
    if !recipe.iter().any(|line| line.contains(gate)) {
        problems.push(format!("the recipe never runs `{gate}`"));
    }
    for line in recipe {
        if line.starts_with('-') || line.starts_with("@-") {
            problems.push(format!("`{line}` ignores errors"));
        }
        for mask in MASKS.iter().filter(|mask| line.contains(**mask)) {
            problems.push(format!("`{line}` contains `{mask}`"));
        }
    }
    problems
}

/// Scenario: the Whitaker and Clippy recipes as written, and ones that mask the gate.
///
/// Invariant: the real recipes carry no existence guard, `|| true`, ignored-error prefix or
/// pipeline; each masking shape is named when it appears.
#[rstest]
#[case::as_written(&["unset CARGO_ENCODED_RUSTFLAGS && RUSTFLAGS=\"-D warnings\" $(WHITAKER) --all -- --workspace"], "")]
#[case::soft_failed(&["$(WHITAKER) --all || true"], "||")]
#[case::ignored(&["-$(WHITAKER) --all"], "ignores errors")]
#[case::guarded(&["command -v $(WHITAKER) && $(WHITAKER) --all"], "command -v")]
#[case::piped(&["$(WHITAKER) --all | tee log"], "|")]
#[case::removed(&["echo skipped"], "never runs")]
fn the_gate_recipe_reader_refuses_masking(
    #[case] recipe: &[&str],
    #[case] fault: &str,
) -> Result<(), String> {
    let problems = masking_problems(recipe, "$(WHITAKER)");
    let named = problems.iter().any(|problem| problem.contains(fault));
    match (fault.is_empty(), problems.is_empty(), named) {
        (true, true, _) | (false, false, true) => Ok(()),
        _ => Err(format!("for {recipe:?} the reader said {problems:?}")),
    }
}

/// Scenario: the Makefile as written.
///
/// Invariant: `lint` lists Clippy then Whitaker, is built serially, and the Whitaker recipe is a
/// plain command that cannot be soft-failed.
#[test]
fn the_makefile_binds_whitaker_into_a_serial_lint() -> Result<(), String> {
    let header = MAKEFILE
        .lines()
        .find(|line| line.starts_with("lint:"))
        .ok_or("no `lint:` rule")?;
    let before_hash = header.split("##").next().unwrap_or(header);
    if before_hash.split_whitespace().collect::<Vec<_>>()
        != ["lint:", "lint-clippy", "lint-whitaker"]
    {
        return Err(format!("`lint` reaches `{before_hash}`"));
    }
    if !MAKEFILE.lines().any(|line| line == ".NOTPARALLEL: lint") {
        return Err("`lint` is not marked `.NOTPARALLEL`".to_owned());
    }
    let whitaker = recipe_of(MAKEFILE, "lint-whitaker").ok_or("`lint-whitaker` has no recipe")?;
    let clippy = recipe_of(MAKEFILE, "lint-clippy").ok_or("`lint-clippy` has no recipe")?;
    let mut problems = masking_problems(&whitaker, "$(WHITAKER)");
    problems.extend(masking_problems(&clippy, "$(CARGO) clippy"));
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{problems:#?}"))
    }
}

/// Returns the lines of the named workflow step, up to the next step.
fn step_of<'a>(workflow: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let header = format!("- name: {name}");
    let mut lines = workflow.lines().skip_while(|line| line.trim() != header);
    lines.next()?;
    Some(
        lines
            .take_while(|line| !line.trim_start().starts_with("- name:"))
            .collect(),
    )
}

/// Scenario: the CI step that runs the gate.
///
/// Invariant: it runs exactly `make lint`, and nothing lets its failure pass; GitHub fails a step
/// whose shell exits non-zero.
#[test]
fn the_ci_lint_step_runs_make_lint_and_cannot_pass_on_failure() -> Result<(), String> {
    let step = step_of(CI_WORKFLOW, "Lint").ok_or("ci.yml has no `Lint` step")?;
    let runs: Vec<&str> = step
        .iter()
        .filter_map(|line| line.trim().strip_prefix("run:"))
        .map(str::trim)
        .collect();
    if runs != ["make lint"] {
        return Err(format!("the Lint step runs {runs:?}"));
    }
    let masked = step.iter().find(|line| {
        line.contains("continue-on-error") || line.contains("||") || line.contains("shell:")
    });
    masked.map_or(Ok(()), |line| {
        Err(format!("the Lint step can mask a failure: `{line}`"))
    })
}
