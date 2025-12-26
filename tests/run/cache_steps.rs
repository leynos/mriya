//! BDD step definitions for cache routing and cache directory management.

use mriya::sync::CACHE_SUBDIRECTORIES;
use rstest_bdd_macros::{given, then};

use super::bdd_steps::{SshLookupDirection, StepError, find_ssh_command, last_ssh_remote_command};
use super::test_helpers::RunContext;

#[given("cache routing is disabled")]
fn cache_routing_disabled(mut run_context: RunContext) -> RunContext {
    run_context.sync_config.route_build_caches = false;
    run_context
}

#[given("cache directory creation is disabled")]
fn cache_directory_creation_disabled(mut run_context: RunContext) -> RunContext {
    run_context.sync_config.create_cache_directories = false;
    run_context
}

#[then("the remote command routes Cargo caches to the volume")]
fn remote_command_routes_cargo_caches(run_context: &RunContext) -> Result<(), StepError> {
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
fn remote_command_does_not_route_cargo_caches(run_context: &RunContext) -> Result<(), StepError> {
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

fn first_ssh_raw_command(run_context: &RunContext) -> Result<String, StepError> {
    find_ssh_command(run_context, SshLookupDirection::First)
}

#[then("the mount command creates cache subdirectories")]
fn mount_command_creates_cache_subdirectories(run_context: &RunContext) -> Result<(), StepError> {
    // The first SSH command is the mount + mkdir cache dirs command
    let mount_command = first_ssh_raw_command(run_context)?;

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
    run_context: &RunContext,
) -> Result<(), StepError> {
    // The first SSH command is the mount command
    let mount_command = first_ssh_raw_command(run_context)?;

    // Verify none of the cache subdirectories are present using the production constant
    for subdir in CACHE_SUBDIRECTORIES {
        let path = format!("/mriya/{subdir}");
        if mount_command.contains(&path) {
            return Err(StepError::Assertion(format!(
                "expected mount command to NOT create cache directory '{path}', got: {mount_command}"
            )));
        }
    }

    Ok(())
}
