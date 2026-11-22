//! Contract for the retired `test-backdoors` feature gate.
//!
//! The fake-run module is private to the binary crate, so an external compile
//! test cannot name it even when its guard is wrong: a trybuild case that
//! referenced `test_backdoors` would fail with or without `cfg(test)`, which
//! proves nothing. This contract therefore asserts the compile-time surface
//! itself — the manifest declares no such feature, every guard in the binary is
//! a test guard, and the fake dispatch sits behind one — and drives the compiler
//! through Cargo to prove that the retired feature is rejected outright.
//! `tests/cli_run.rs` supplies the behavioural half against the shipped binary.
//!
//! The retired feature name is assembled rather than spelled, so this file
//! cannot satisfy its own absence check.

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};

/// The retired Cargo feature, never to return.
const RETIRED_FEATURE: &str = concat!("test", "-backdoors");

/// The binary module that implements the fake run modes.
const FAKE_RUN_MODULE: &str = concat!("test", "_backdoors");

/// The call that performs a fake outcome, identified by its opening paren so it
/// cannot match the import that names the same function.
const DISPATCH_CALL: &str = "select_fake_outcome(";

/// Reads one repository file through a capability handle rooted at the
/// manifest directory.
fn read(relative: &str) -> Result<String, String> {
    let manifest = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Dir::open_ambient_dir(manifest, ambient_authority())
        .map_err(|error| format!("{manifest} should open: {error}"))?;
    dir.read_to_string(relative)
        .map_err(|error| format!("{manifest}/{relative} should read: {error}"))
}

/// Fails unless `haystack` contains `needle`.
///
/// Every caller asserts a fact that must be present, so a renamed or moved
/// file cannot make the contract pass by finding nothing.
fn require_contains(haystack: &str, needle: &str, what: &str) -> Result<(), String> {
    if haystack.contains(needle) {
        Ok(())
    } else {
        Err(format!("{what} is missing: expected `{needle}`"))
    }
}

#[test]
fn the_manifest_declares_no_retired_feature() -> Result<(), String> {
    let manifest = read("Cargo.toml")?;

    if manifest.contains(RETIRED_FEATURE) {
        return Err(format!(
            "Cargo.toml names `{RETIRED_FEATURE}`; the fake run modes must compile only under \
             cfg(test), so no build flag may enable them"
        ));
    }
    require_contains(&manifest, "[package]", "the package manifest")?;
    Ok(())
}

#[test]
fn the_fake_run_module_is_guarded_by_cfg_test_alone() -> Result<(), String> {
    let source = read("src/main.rs")?;

    require_contains(
        &source,
        &format!("#[cfg(test)]\nmod {FAKE_RUN_MODULE};"),
        "the module declaration guard",
    )?;
    require_contains(
        &source,
        &format!("#[cfg(test)]\nuse {FAKE_RUN_MODULE}::"),
        "the module import guard",
    )?;
    if source.contains(RETIRED_FEATURE) {
        return Err(format!(
            "src/main.rs names `{RETIRED_FEATURE}`; the guard must be `#[cfg(test)]` alone"
        ));
    }

    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let is_condition = trimmed.starts_with("#[cfg(") || trimmed.starts_with("#[cfg_attr(");
        if is_condition && !trimmed.contains("test") {
            return Err(format!(
                "src/main.rs:{}: `{trimmed}` is not a test guard",
                index + 1
            ));
        }
    }
    Ok(())
}

/// Returns the source of `run_command` alone.
///
/// Scoping the guard search to the function is what stops an unrelated test
/// guard elsewhere in the file from satisfying the check. The slice runs from
/// the item to the next closing brace in column zero, which `cargo fmt` fixes.
fn run_command_body(source: &str) -> Result<&str, String> {
    let start = source
        .find("async fn run_command(")
        .ok_or_else(|| String::from("the run entry point is missing"))?;
    let item = source
        .get(start..)
        .ok_or_else(|| format!("the run entry point at byte {start} is not addressable"))?;
    let end = item
        .find("\n}\n")
        .ok_or_else(|| String::from("the run entry point has no closing brace in column zero"))?;
    item.get(..end)
        .ok_or_else(|| format!("the run entry point body at byte {end} is not addressable"))
}

#[test]
fn the_fake_dispatch_sits_behind_a_test_guard() -> Result<(), String> {
    let source = read("src/main.rs")?;
    let body = run_command_body(&source)?;

    let sites = body.matches(DISPATCH_CALL).count();
    if sites != 1 {
        return Err(format!(
            "expected exactly one fake dispatch call in run_command, found {sites}"
        ));
    }
    let guards = body.matches("#[cfg(").count();
    if guards != 1 {
        return Err(format!(
            "expected exactly one cfg guard in run_command, found {guards}"
        ));
    }
    let guard_at = body
        .find("#[cfg(test)]")
        .ok_or_else(|| String::from("run_command has no `#[cfg(test)]` guard"))?;
    let dispatch_at = body
        .find(DISPATCH_CALL)
        .ok_or_else(|| format!("the fake dispatch call `{DISPATCH_CALL}` is missing"))?;
    if dispatch_at < guard_at {
        return Err(String::from(
            "the fake dispatch runs before the test guard, so it is unguarded",
        ));
    }
    let between = body
        .get(guard_at..dispatch_at)
        .ok_or_else(|| String::from("the guarded region is not addressable"))?;
    if between.contains('}') {
        return Err(String::from(
            "the test guard's block closes before the fake dispatch, so the dispatch is unguarded",
        ));
    }
    if !between
        .trim_start_matches("#[cfg(test)]")
        .trim_start()
        .starts_with('{')
    {
        return Err(String::from(
            "the test guard does not open the block containing the fake dispatch",
        ));
    }
    Ok(())
}

/// Cargo must reject the retired feature, so no build flag can enable the fake
/// run modes.
///
/// This is the executable compile-time check: the source checks above could all
/// hold while a feature of this name was quietly reintroduced, and a trybuild
/// case cannot reach the module because it is private to the binary crate. The
/// build fails during resolution, before any compilation, so it needs no warm-up
/// and no network.
#[test]
fn cargo_rejects_the_retired_feature() -> Result<(), String> {
    let requested = format!("--features={RETIRED_FEATURE}");
    let output = std::process::Command::new(env!("CARGO"))
        .current_dir(Utf8Path::new(env!("CARGO_MANIFEST_DIR")))
        .env("CARGO_NET_OFFLINE", "true")
        .args(["build", "--bin", "mriya", requested.as_str()])
        .output()
        .map_err(|error| format!("cargo should start: {error}"))?;

    if output.status.success() {
        return Err(format!(
            "cargo accepted `{RETIRED_FEATURE}`; the feature must not return"
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expected = format!("does not contain this feature: {RETIRED_FEATURE}");
    if stderr.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "cargo failed without naming the retired feature; stderr: {stderr}"
        ))
    }
}
