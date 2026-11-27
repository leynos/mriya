//! BDD step definitions for cache routing and cache directory management.

use mriya::sync::CACHE_SUBDIRECTORIES;
use rstest_bdd_macros::{given, then};

use super::{
    bdd_steps::{StepError, context, last_ssh_remote_command},
    invocation_steps::{InvocationRole, classify_invocation},
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

#[given("cache directory creation fails")]
fn cache_directory_creation_fails(
    run_context_result: RunContextResult,
) -> Result<RunContextResult, StepError> {
    let run_context = run_context_result?;
    // The mount response is queued first; this failure is consumed by the
    // cache-directory command that follows it.
    run_context
        .runner
        .push_output(Some(1), "", "mkdir: permission denied");
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
        // A verified cache fails closed: the command only runs when the mount
        // path's source is still the device the mount step verified.
        "if [ \"$(findmnt -no SOURCE --mountpoint /mriya 2>/dev/null)\" != /dev/sdb ]; then",
        "refusing to run",
        "export CARGO_HOME=/mriya/cargo",
        "export RUSTUP_HOME=/mriya/rustup",
        "export CARGO_TARGET_DIR=/mriya/target",
        "export GOMODCACHE=/mriya/go/pkg/mod",
        "export GOCACHE=/mriya/go/build-cache",
        "export PIP_CACHE_DIR=/mriya/pip/cache",
        "export npm_config_cache=/mriya/npm/cache",
        "export YARN_CACHE_FOLDER=/mriya/yarn/cache",
        "export PNPM_STORE_PATH=/mriya/pnpm/store",
        "cd /remote",
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
    cache_directory_payloads(run_context)
        .into_iter()
        .next()
        .ok_or_else(|| {
            StepError::Assertion(String::from(
                "missing cache-directory invocation with mkdir and a known subdirectory",
            ))
        })
}

/// Returns the payloads classified as automatic cache-directory calls.
///
/// The requested command is excluded by its role, so a user command such as
/// `mkdir -p /mriya/target` is never mistaken for cache setup.
fn cache_directory_payloads(run_context: &RunContext) -> Vec<String> {
    let ssh_bin = run_context.sync_config.ssh_bin.as_str();
    let Some(command) = run_context.requested_command.as_deref() else {
        return Vec::new();
    };
    run_context
        .runner
        .invocations()
        .into_iter()
        .filter(|invocation| invocation.program == ssh_bin)
        .filter(|invocation| {
            classify_invocation(invocation, ssh_bin, command) == InvocationRole::CacheDirectories
        })
        .filter_map(|invocation| invocation.args.last().cloned())
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// Returns the first SSH payload that creates a known cache subdirectory.
fn offending_cache_directory(run_context: &RunContext) -> Option<String> {
    cache_directory_payloads(run_context).into_iter().next()
}

/// Reports whether the directory step verifies the mounted device.
fn cache_step_checks_device(command: &str) -> bool {
    command.contains("findmnt -no SOURCE --mountpoint /mriya")
        && command.contains("/dev/sdb")
        && command.contains("exit 1")
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

    // A verified cache is required: directory creation must fail unless the
    // mount path is still backed by the verified device.
    if !cache_step_checks_device(&mount_command) {
        return Err(StepError::Assertion(format!(
            "expected mkdir to fail unless the verified device is mounted, got: {mount_command}"
        )));
    }

    Ok(())
}

#[then("the mount command does not create cache subdirectories")]
fn mount_command_does_not_create_cache_subdirectories(
    run_context_result: &RunContextResult,
) -> Result<(), StepError> {
    let run_context = context(run_context_result)?;
    if let Some(payload) = offending_cache_directory(run_context) {
        return Err(StepError::Assertion(format!(
            "expected no invocation to create cache directories, got: {payload}"
        )));
    }
    Ok(())
}
