//! Cases for the build-tools contract: the routing reader against text it must accept and refuse, the
//! preflight and installer scripts against each tool a developer may lack, and the real Make routes
//! run with a missing or complete toolset. Case names are distinct, so a run can list and count them.

use rstest::rstest;

use super::{
    build_tools_routes::{EXCLUDED_ROUTES, PREFLIGHT_ROUTES, route_problems, routing_problems},
    build_tools_scripts::{
        Toolset, cargo_calls, expect_actionable_failure, run_preflight, run_route, sandbox_with,
    },
    config::TOOLCHAIN,
    installer::{Digest, installed_linker, published, run_installer, rustup_calls},
    make::Host,
    process::probe_make,
    sandbox::{Sandbox, is_tool},
};

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
#[rstest]
#[case::complete(Toolset::COMPLETE, &[])]
#[case::no_linker(Toolset { linker: None, ..Toolset::COMPLETE }, &["mold linker not found", "make install-build-tools"])]
#[case::drifted_linker(Toolset { linker: Some("2.40.4"), ..Toolset::COMPLETE }, &["does not match the pin 2.41.0", "make install-build-tools"])]
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
    expect_actionable_failure(&output, fragments)
}

/// Scenario: a development route run for real, with and without the tools it needs.
///
/// Invariant: with a missing `mold` the route fails and the fake Cargo is never started, so there is no
/// silent fallback to another linker; with a complete toolset the route reaches Cargo.
#[rstest]
#[case::make_build("build")]
#[case::make_test("test")]
#[case::make_lint("lint")]
#[case::make_typecheck("typecheck")]
#[case::make_scaleway_test("scaleway-test")]
#[case::make_scaleway_janitor("scaleway-janitor")]
fn a_missing_tool_stops_every_development_route_before_cargo(
    #[case] route: &str,
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
#[case::make_build("build")]
#[case::make_test("test")]
#[case::make_typecheck("typecheck")]
#[case::make_scaleway_janitor("scaleway-janitor")]
fn a_complete_toolset_lets_a_development_route_reach_cargo(
    #[case] route: &str,
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
    let output = run_route(&sandbox, "release")?;
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

/// Scenario: the installer against a published archive, a tampered checksum, and a mold linker already present.
///
/// Invariant: an honest archive is unpacked and the pinned toolchain is requested with every component
/// `rust-toolchain.toml` lists; a checksum mismatch installs nothing and requests no toolchain; a mold linker
/// already at the pin is not downloaded again.
#[test]
fn the_installer_unpacks_a_verified_archive_and_requests_the_pinned_toolchain() -> Result<(), String>
{
    let sandbox = published(Digest::Honest)?;
    let output = run_installer(&sandbox)?;
    if !output.status.success() {
        return Err(format!(
            "installer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !installed_linker(&sandbox)? {
        return Err("the archive was not unpacked into the prefix".to_owned());
    }
    let calls = rustup_calls(&sandbox)?.unwrap_or_default();
    let expected = "toolchain install nightly-2026-11-06 --profile minimal \
                    --component rustfmt --component clippy --component rust-analyzer";
    if calls.trim() == expected {
        Ok(())
    } else {
        Err(format!("rustup was asked `{calls}`, expected `{expected}`"))
    }
}

#[test]
fn the_installer_refuses_a_checksum_mismatch_and_installs_nothing() -> Result<(), String> {
    let sandbox = published(Digest::Tampered)?;
    let output = run_installer(&sandbox)?;
    expect_actionable_failure(&output, &["checksum mismatch", "refusing to install"])?;
    if installed_linker(&sandbox)? || rustup_calls(&sandbox)?.is_some() {
        return Err("a refused archive still installed something".to_owned());
    }
    Ok(())
}

#[test]
fn the_installer_leaves_a_linker_already_at_the_pin_alone() -> Result<(), String> {
    let sandbox = published(Digest::Tampered)?;
    sandbox.write_tool("mold", "echo 'mold 2.41.0 (compatible with GNU ld)'")?;
    let output = run_installer(&sandbox)?;
    if !output.status.success() {
        return Err(format!(
            "installer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // The checksum is tampered, so any download attempt would have failed the run above.
    if installed_linker(&sandbox)? {
        return Err("a mold linker already at the pin was downloaded again".to_owned());
    }
    Ok(())
}

/// Scenario: the toolchain file the installer reads.
///
/// Invariant: `rust-analyzer` joins `rustfmt` and `clippy`, and the pin stays a dated nightly.
#[test]
fn the_toolchain_file_lists_language_server_beside_rustfmt_and_clippy() -> Result<(), String> {
    let components = TOOLCHAIN
        .lines()
        .find(|line| line.trim_start().starts_with("components"))
        .ok_or("rust-toolchain.toml has no components line")?;
    for name in ["rustfmt", "clippy", "rust-analyzer"] {
        if !components.contains(&format!("\"{name}\"")) {
            return Err(format!("{name} is missing from `{components}`"));
        }
    }
    if TOOLCHAIN.contains("nightly-2026-11-06") {
        Ok(())
    } else {
        Err("the nightly pin moved".to_owned())
    }
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
    let found = is_tool(&tools.root().join("bin"), name);
    if found == expected {
        Ok(())
    } else {
        Err(format!("is_tool({name}) was {found}, expected {expected}"))
    }
}
