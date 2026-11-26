//! BDD step definitions for cache routing and cache directory management.

use mriya::sync::CACHE_SUBDIRECTORIES;
use rstest_bdd_macros::{given, then};

use super::{
    bdd_steps::{StepError, context, last_ssh_remote_command},
    test_helpers::{RunContext, RunContextResult},
};

#[given("cache routing is disabled")]
fn cache_routing_disabled(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let mut run_context = run_context_result?;
    run_context.sync_config.route_build_caches = false;
    Ok(Ok(run_context))
}

#[given("cache directory creation is disabled")]
fn cache_directory_creation_disabled(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let mut run_context = run_context_result?;
    run_context.sync_config.create_cache_directories = false;
    Ok(Ok(run_context))
}

#[then("the remote command routes Cargo caches to the volume")]
fn remote_command_routes_cargo_caches(
    run_context_result: &RunContextResult,
) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    let remote_command = last_ssh_remote_command(run_context)?;
    for required in [
        "if mountpoint -q /mriya 2>/dev/null; then",
        "export CARGO_HOME=/mriya/cargo",
        "export RUSTUP_HOME=/mriya/rustup",
        "export CARGO_TARGET_DIR=/mriya/target",
        "export GOMODCACHE=/mriya/go/pkg/mod",
        "export GOCACHE=/mriya/go/build-cache",
        "export PIP_CACHE_DIR=/mriya/pip/cache",
        "export npm_config_cache=/mriya/npm/cache",
        "export YARN_CACHE_FOLDER=/mriya/yarn/cache",
        "export PNPM_STORE_PATH=/mriya/pnpm/store",
        "fi; cd",
    ] {
        if !remote_command.contains(required) {
            return Err(StepError::Assertion(format!(
                "expected remote command to include '{required}', got: {remote_command}"
            )));
        }
    }
    Ok(())
}

#[then("the remote command does not route Cargo caches")]
fn remote_command_does_not_route_cargo_caches(
    run_context_result: &RunContextResult,
) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    let remote_command = last_ssh_remote_command(run_context)?;

    const CARGO_CACHE_VARS: &[&str] = &["CARGO_TARGET_DIR=", "CARGO_HOME=", "RUSTUP_HOME="];

    if CARGO_CACHE_VARS
        .iter()
        .any(|var| remote_command.contains(var))
    {
        return Err(StepError::Assertion(format!(
            "expected remote command to avoid cache routing, got: {remote_command}"
        )));
    }
    Ok(())
}

/// Returns the SSH payload that creates cache subdirectories.
///
/// The mount script and the directory creation are separate SSH calls now, so
/// the step cannot assume the first SSH invocation is the one it inspects.
fn cache_directories_command(run_context: &RunContext) -> Result<String, StepError> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let subdir = CACHE_SUBDIRECTORIES
        .first()
        .ok_or_else(|| StepError::Assertion(String::from("no cache subdirectories")))?;
    let expected = format!("/mriya/{subdir}");
    run_context
        .runner
        .invocations()
        .into_iter()
        .rev()
        .find(|invocation| {
            invocation.program == ssh_bin
                && invocation
                    .args
                    .last()
                    .is_some_and(|arg| arg.to_string_lossy().contains(&expected))
        })
        .and_then(|invocation| {
            invocation
                .args
                .last()
                .map(|arg| arg.to_string_lossy().into_owned())
        })
        .ok_or_else(|| {
            StepError::Assertion(format!(
                "missing cache-directory invocation containing {expected}"
            ))
        })
}

#[then("the mount command creates cache subdirectories")]
fn mount_command_creates_cache_subdirectories(
    run_context_result: &RunContextResult,
) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    let mount_command = cache_directories_command(run_context)?;

    // Verify the mkdir -p command is present
    if !mount_command.contains("mkdir -p") {
        return Err(StepError::Assertion(format!(
            "expected mount command to include 'mkdir -p', got: {mount_command}"
        )));
    }

    // Verify all cache subdirectories are present using the production constant
    for subdir in CACHE_SUBDIRECTORIES {
        let expected = format!("/mriya/{subdir}");
        if !mount_command.contains(&expected) {
            return Err(StepError::Assertion(format!(
                "expected mount command to include '{expected}', got: {mount_command}"
            )));
        }
    }

    // Verify the mkdir is gated by a mountpoint check
    if !mount_command.contains("if mountpoint -q /mriya") {
        return Err(StepError::Assertion(format!(
            "expected mkdir to be gated by mountpoint check, got: {mount_command}"
        )));
    }

    Ok(())
}

#[then("the mount command does not create cache subdirectories")]
fn mount_command_does_not_create_cache_subdirectories(
    run_context_result: &RunContextResult,
) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    // Verify no SSH invocation creates a cache subdirectory, using the
    // production constant.
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    for invocation in run_context.runner.invocations() {
        if invocation.program != ssh_bin {
            continue;
        }
        let Some(payload) = invocation
            .args
            .last()
            .map(|arg| arg.to_string_lossy().into_owned())
        else {
            continue;
        };
        // The routing preamble exports these paths without creating them; only
        // a command that also creates directories is a violation.
        if !payload.contains("mkdir -p") {
            continue;
        }
        for subdir in CACHE_SUBDIRECTORIES {
            let path = format!("/mriya/{subdir}");
            if payload.contains(&path) {
                return Err(StepError::Assertion(format!(
                    "expected no invocation to create cache directory '{path}', got: {payload}"
                )));
            }
        }
    }

    Ok(())
}
