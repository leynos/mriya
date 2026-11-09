//! Cases for the preflight half of the build-tools contract: the routing reader against text it must
//! accept and refuse, the preflight against each tool a developer may lack, and the real Make routes
//! run with a missing or complete toolset. Case names are distinct, so a run can list and count them.

use rstest::rstest;

use super::{
    build_tools_routes::{EXCLUDED_ROUTES, PREFLIGHT_ROUTES, route_problems, routing_problems},
    build_tools_scripts::{
        Toolset, cargo_calls, expect_actionable_failure, run_preflight, run_route, sandbox_with,
    },
    make::{Host, Target},
    process::probe_make,
    sandbox::{Sandbox, is_tool},
};

#[cfg(target_os = "linux")]
use super::build_tools_scripts::Linker;
#[cfg(target_os = "linux")]
use super::pins::pinned_linker;

/// Scenario: the evaluated Make invocation for every route, on a Linux and a macOS host.
///
/// Invariant: each development route prints the preflight before its first Cargo command, the release
/// route prints none, no command hard-codes `cargo`, and the count proves the routes were read.
#[rstest]
#[case::linux(Host::Linux)]
#[case::darwin(Host::Darwin)]
fn every_route_is_ordered_by_the_evaluated_make_invocation(
    #[case] host: Host,
) -> Result<(), String> {
    let (problems, read) = routing_problems(probe_make, host)?;
    if !problems.is_empty() {
        return Err(format!("{problems:#?}"));
    }
    let expected = PREFLIGHT_ROUTES.len() + EXCLUDED_ROUTES.len();
    if read == expected {
        Ok(())
    } else {
        Err(format!("read {read} routes, expected {expected}"))
    }
}

/// Scenario: dry-run text a route must be refused for.
///
/// Invariant: a missing preflight, a preflight after Cargo, a hard-coded `cargo` (also inside a quoted
/// `trap`), a route that prints no Cargo command, and a release route that demands the preflight each
/// name their fault; the well-formed text names none.
#[rstest]
#[case::well_formed(
    "scripts/check-build-tools.sh\nprobe-cargo fetch\nRUSTFLAGS=\"-D warnings\" probe-cargo test\n",
    true,
    ""
)]
#[case::missing_preflight("probe-cargo test\n", true, "never reaches")]
#[case::preflight_after_cargo(
    "probe-cargo fetch\nscripts/check-build-tools.sh\n",
    true,
    "before the preflight"
)]
#[case::hard_coded_cargo("scripts/check-build-tools.sh\ncargo test\n", true, "ignores $(CARGO)")]
#[case::hard_coded_in_a_trap(
    "scripts/check-build-tools.sh\nprobe-cargo run; trap 'cargo run' EXIT\n",
    true,
    "ignores $(CARGO)"
)]
#[case::no_cargo_at_all("scripts/check-build-tools.sh\n", true, "reads nothing")]
#[case::release_demands_the_preflight(
    "scripts/check-build-tools.sh\nprobe-cargo build --release\n",
    false,
    "must not require"
)]
#[case::release_without_it("RUSTFLAGS=\"${RUSTFLAGS-}\" probe-cargo build --release\n", false, "")]
fn the_route_reader_accepts_and_refuses(
    #[case] stdout: &str,
    #[case] needs_preflight: bool,
    #[case] fault: &str,
) -> Result<(), String> {
    let problems = route_problems("route", stdout, needs_preflight);
    let named = problems.iter().any(|problem| problem.contains(fault));
    match (fault.is_empty(), problems.is_empty(), named) {
        (true, true, _) | (false, false, true) => Ok(()),
        _ => Err(format!("for `{stdout}` the reader said {problems:?}")),
    }
}

/// Scenario: the preflight script run against a toolset missing something.
///
/// Invariant: a gap fails with a message that names the gap and the command that fixes it, and never
/// falls back; a complete toolset passes.
#[cfg(target_os = "linux")]
#[rstest]
#[case::complete(Toolset::COMPLETE, &[])]
#[case::no_linker(Toolset { linker: None, ..Toolset::COMPLETE }, &["mold linker not found", "make install-build-tools"])]
#[case::drifted_linker(Toolset { linker: Some(Linker::Drifted("0.0.0")), ..Toolset::COMPLETE }, &["does not match the pin", "make install-build-tools"])]
#[case::no_c_compiler(Toolset { cc_links: None, ..Toolset::COMPLETE }, &["no C compiler"])]
#[case::cc_rejects_the_linker(Toolset { cc_links: Some(false), ..Toolset::COMPLETE }, &["cannot link with -fuse-ld=mold"])]
#[case::no_toolchain(Toolset { has_toolchain: false, ..Toolset::COMPLETE }, &["is not installed", "make install-build-tools"])]
fn the_preflight_reports_each_gap_with_its_remedy(
    #[case] toolset: Toolset,
    #[case] fragments: &[&str],
) -> Result<(), String> {
    let output = run_preflight(&sandbox_with(toolset)?)?;
    if fragments.is_empty() {
        return if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "a complete toolset failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
        };
    }
    let mut expected: Vec<&str> = fragments.to_vec();
    // The drifted case must name the pin, which lives in `tools/mold/VERSION`.
    let drift = format!("does not match the pin {}", pinned_linker());
    if matches!(toolset.linker, Some(Linker::Drifted(_))) {
        expected.push(&drift);
    }
    expect_actionable_failure(&output, &expected)
}

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

/// Scenario: the preflight on a host that is not Linux.
///
/// Invariant: the linker and driver checks are skipped with a note, not failed, because the host
/// keeps its platform linker.
#[test]
fn a_non_linux_host_skips_the_linker_checks() -> Result<(), String> {
    let sandbox = sandbox_with(Toolset {
        linker: None,
        cc_links: None,
        ..Toolset::COMPLETE
    })?;
    sandbox.pretend_darwin()?;
    let output = run_preflight(&sandbox)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "the preflight failed without a linker on a non-Linux host: {stderr}"
        ));
    }
    if !stderr.contains("Linux-only") {
        return Err(format!(
            "the preflight never said the linker is Linux-only: {stderr}"
        ));
    }
    if stderr.contains("mold linker not found") {
        return Err(format!("the preflight checked the linker anyway: {stderr}"));
    }
    Ok(())
}

/// Scenario: the preflight on a host that is not Linux and has no pinned toolchain either.
///
/// Invariant: skipping the linker checks does not skip the toolchain check, so a non-Linux host
/// fails with the toolchain diagnostic and its remedy while still saying the linker is Linux-only.
#[test]
fn a_non_linux_host_still_fails_without_the_pinned_toolchain() -> Result<(), String> {
    let sandbox = sandbox_with(Toolset {
        linker: None,
        cc_links: None,
        has_toolchain: false,
    })?;
    sandbox.pretend_darwin()?;
    let output = run_preflight(&sandbox)?;
    expect_actionable_failure(&output, &["is not installed", "make install-build-tools"])?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("Linux-only") {
        return Err(format!(
            "the preflight never said the linker is Linux-only: {stderr}"
        ));
    }
    if stderr.contains("mold linker not found") {
        return Err(format!("the preflight checked the linker anyway: {stderr}"));
    }
    Ok(())
}

/// Checks that a lookup reports absence only for a path that is missing, and that every other
/// failure is an error naming the path.
///
/// `present` is what the lookup answered for a path that does not exist; `unreadable` is its
/// answer for a path the filesystem refuses to read for some reason other than absence.
fn expect_only_absence_reads_as_missing<T>(
    path: &str,
    present: bool,
    unreadable: Result<T, String>,
) -> Result<(), String> {
    if present {
        return Err(format!("{path} read as present"));
    }
    match unreadable {
        Ok(_) => Err(format!("{path} read as usable")),
        Err(reason) if reason.contains(path) => Ok(()),
        Err(reason) => Err(format!("the error does not name {path}: {reason}")),
    }
}

/// Scenario: a sandbox read of a path that is absent and one that exists but is not a file.
///
/// Invariant: only a genuinely missing path reads as absent; any other failure is an error naming the
/// path, so a contract cannot pass by mistaking an unreadable file for one that was never written.
#[test]
fn a_sandbox_read_reports_a_failure_it_cannot_attribute_to_absence() -> Result<(), String> {
    let sandbox = Sandbox::new()?;
    sandbox.make_dir("a-directory")?;
    expect_only_absence_reads_as_missing(
        "a-directory",
        sandbox.read("missing")?.is_some(),
        sandbox.read("a-directory"),
    )
}

/// Scenario: the release route with no `mold` at all.
///
/// Invariant: a shipped artefact needs none of the development tools, so it neither runs the preflight
/// nor fails for their absence.
#[test]
fn the_release_route_needs_no_development_tool() -> Result<(), String> {
    let sandbox = sandbox_with(Toolset {
        linker: None,
        cc_links: None,
        has_toolchain: false,
    })?;
    let output = run_route(&sandbox, Target("release"))?;
    if !output.status.success() {
        return Err(format!(
            "`make release` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("build-tools:") || cargo_calls(&sandbox)?.is_empty() {
        return Err(format!("release ran the preflight or no Cargo: {stderr}"));
    }
    Ok(())
}

/// Scenario: a tool directory whose entries are real files and symlinks of every shape a host has.
///
/// Invariant: a tool is found through an absolute link that escapes its directory (as a multicall
/// coreutils install does), through a `../` link, and through a chain of links; a dangling link, a
/// loop, and a link to a directory are not tools. The sandbox's capability handle refuses the first
/// three, which is why the lookup reads links itself.
#[rstest]
#[case::regular_file("regular", true)]
#[case::absolute_escaping_link("absolute", true)]
#[case::relative_escaping_link("relative", true)]
#[case::chained_links("chain", true)]
#[case::dangling_link("dangling", false)]
#[case::link_loop("loop", false)]
#[case::link_to_a_directory("directory", false)]
fn a_tool_is_found_through_any_symlink_that_reaches_a_file(
    #[case] name: &str,
    #[case] expected: bool,
) -> Result<(), String> {
    let outside = Sandbox::new()?;
    outside.write_file("real", b"#!/bin/sh\n")?;
    outside.make_dir("subdir")?;
    let tools = Sandbox::new()?;
    tools.write_file("bin/regular", b"x")?;
    let real = outside.root().join("real");
    tools.link("bin/absolute", real.as_str())?;
    // Climb from `<root>/bin` to `/` however deep the temporary directory is, then descend.
    let climb = "../".repeat(tools.root().components().count());
    let hop = format!("{climb}{}", real.as_str().trim_start_matches('/'));
    tools.link("bin/relative", hop)?;
    tools.link("bin/chain", "absolute")?;
    tools.link("bin/dangling", "../nowhere")?;
    tools.link("bin/loop", "loop")?;
    tools.link("bin/directory", outside.root().join("subdir").as_str())?;
    let found = is_tool(&tools.root().join("bin"), name)?;
    if found == expected {
        Ok(())
    } else {
        Err(format!("is_tool({name}) was {found}, expected {expected}"))
    }
}

/// Scenario: a tool lookup below a directory that links to itself, so opening it fails.
///
/// Invariant: only a path that is genuinely absent reads as "no tool"; a filesystem that cannot
/// answer is an error naming the path, so a lookup cannot pass by mistaking an unreadable tool for
/// one that was never installed.
#[test]
fn a_tool_lookup_reports_a_failure_it_cannot_attribute_to_absence() -> Result<(), String> {
    let sandbox = Sandbox::new()?;
    sandbox.link("bin/loop", "loop")?;
    let bin = sandbox.root().join("bin");
    expect_only_absence_reads_as_missing(
        "bin/loop",
        is_tool(&bin, "missing")?,
        is_tool(&bin.join("loop"), "mold"),
    )
}
