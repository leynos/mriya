//! Cases that drive the real Make targets: each development route with a missing and a complete
//! toolset, the prefix outranking an inherited `PATH`, and the release route needing no development
//! tool at all. They live apart from the preflight cases because they run `make` and so depend on
//! the host's own tools, while those cases only judge the scripts.

use rstest::rstest;

use super::{
    build_tools_scripts::{
        Toolset, cargo_calls, expect_actionable_failure, run_route, sandbox_with,
    },
    make::Target,
};

#[cfg(target_os = "linux")]
use super::build_tools_scripts::Linker;
#[cfg(target_os = "linux")]
use super::pins::pinned_linker;

/// Scenario: a development route run for real, with and without the tools it needs.
///
/// Invariant: with a missing `mold` the route fails and the fake Cargo is never started, so there is no
/// silent fallback to another linker; with a complete toolset the route reaches Cargo.
#[cfg(target_os = "linux")]
#[rstest]
#[case::make_build(Target("build"))]
#[case::make_test(Target("test"))]
#[case::make_lint(Target("lint"))]
#[case::make_typecheck(Target("typecheck"))]
#[case::make_scaleway_test(Target("scaleway-test"))]
#[case::make_scaleway_janitor(Target("scaleway-janitor"))]
fn a_missing_tool_stops_every_development_route_before_cargo(
    #[case] route: Target<'static>,
) -> Result<(), String> {
    let sandbox = sandbox_with(Toolset {
        linker: None,
        ..Toolset::COMPLETE
    })?;
    let output = run_route(&sandbox, route)?;
    expect_actionable_failure(
        &output,
        &["mold linker not found", "make install-build-tools"],
    )?;
    let calls = cargo_calls(&sandbox)?;
    if calls.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "`make {route}` ran Cargo past a failed preflight: {calls:?}"
        ))
    }
}

#[rstest]
#[case::make_build(Target("build"))]
#[case::make_test(Target("test"))]
#[case::make_typecheck(Target("typecheck"))]
#[case::make_scaleway_janitor(Target("scaleway-janitor"))]
fn a_complete_toolset_lets_a_development_route_reach_cargo(
    #[case] route: Target<'static>,
) -> Result<(), String> {
    let sandbox = sandbox_with(Toolset::COMPLETE)?;
    let output = run_route(&sandbox, route)?;
    if !output.status.success() {
        return Err(format!(
            "`make {route}` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if cargo_calls(&sandbox)?.is_empty() {
        return Err(format!("`make {route}` succeeded without running Cargo"));
    }
    Ok(())
}

/// Scenario: a route whose inherited `PATH` resolves a drifted `mold` before the installed prefix.
///
/// Invariant: the Makefile's prefix outranks the inherited `PATH`, so the route uses the pinned
/// release the installer unpacked rather than a distribution one earlier in the caller's `PATH`.
#[cfg(target_os = "linux")]
#[test]
fn a_development_route_prefers_the_installed_prefix_over_the_inherited_path() -> Result<(), String>
{
    let sandbox = sandbox_with(Toolset {
        linker: Some(Linker::Drifted("0.0.0")),
        ..Toolset::COMPLETE
    })?;
    sandbox.write_tool(
        "pinned-mold",
        format!(
            "echo 'mold {pin} (compatible with GNU ld)'",
            pin = pinned_linker()
        ),
    )?;
    sandbox.make_dir("prefix/bin")?;
    sandbox.link("prefix/bin/mold", "../../bin/pinned-mold")?;
    let output = run_route(&sandbox, Target("test"))?;
    if !output.status.success() {
        return Err(format!(
            "`make test` failed although the prefix holds the pinned release: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if cargo_calls(&sandbox)?.is_empty() {
        return Err("`make test` succeeded without running Cargo".to_owned());
    }
    Ok(())
}
