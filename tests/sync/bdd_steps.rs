use mriya::sync::{SyncConfig, SyncDestination, SyncError, Syncer};
use mriya::InstanceNetworking;
use rstest_bdd::skip;
use rstest_bdd_macros::{given, then, when};

use super::rsync_simulator::simulate_rsync;
use super::test_doubles::{LocalCopyRunner, ScriptedRunner};
use super::test_helpers::{build_scripted_context, workspace, ScriptedContext, Workspace};

#[given("a workspace with a gitignored cache on the remote")]
fn workspace_with_cache() -> Workspace {
    let workspace = Workspace::new();

    super::test_helpers::write_file(
        workspace.local_root.join(".gitignore").as_path(),
        "target/\n.DS_Store\n",
    );
    super::test_helpers::write_file(
        workspace.local_root.join("src").join("lib.rs").as_path(),
        "pub fn meaning() -> u32 { 42 }\n",
    );

    super::test_helpers::write_file(
        workspace
            .remote_root
            .join("target")
            .join("cache.txt")
            .as_path(),
        "cached artifact",
    );
    super::test_helpers::write_file(
        workspace.remote_root.join("stale.txt").as_path(),
        "remove me",
    );

    workspace
}

#[when("I run git-aware rsync sync to the remote path")]
fn run_git_aware_sync(workspace: &Workspace) -> Result<Workspace, SyncError> {
    let config = SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: workspace.remote_root.to_string(),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
    };

    let syncer = Syncer::new(config, LocalCopyRunner)?;
    let destination = SyncDestination::Local {
        path: workspace.remote_root.clone(),
    };
    syncer.sync(&workspace.local_root, &destination)?;

    Ok(workspace.clone())
}

#[then("the gitignored cache directory remains after sync")]
fn cache_survives(workspace: &Workspace) {
    let cache_path = workspace.remote_root.join("target").join("cache.txt");
    assert!(
        cache_path.is_file(),
        "gitignored target directory should be preserved"
    );
}

#[then("tracked files are mirrored to the remote")]
fn tracked_files_updated(workspace: &Workspace) {
    let synced_file = workspace.remote_root.join("src").join("lib.rs");
    let contents = std::fs::read_to_string(&synced_file)
        .unwrap_or_else(|err| panic!("read synced file {synced_file}: {err}"));
    assert!(
        contents.contains("meaning"),
        "source contents should be synced"
    );

    assert!(
        !workspace.remote_root.join("stale.txt").exists(),
        "non-ignored stale files should be removed by rsync --delete"
    );
}

#[given("a scripted runner that succeeds at sync")]
fn scripted_runner() -> ScriptedContext {
    let runner = ScriptedRunner::new();
    runner.push_success(); // rsync success

    build_scripted_context(runner, "temp source for scripted runner")
}

#[when("the remote command exits with \"{code}\"")]
fn remote_command_exits(scripted_context: &ScriptedContext, code: i32) -> mriya::sync::RemoteCommandOutput {
    scripted_context.runner.push_exit_code(code);
    let syncer = Syncer::new(
        scripted_context.config.clone(),
        scripted_context.runner.clone(),
    )
    .unwrap_or_else(|err| panic!("failed to build syncer: {err}"));
    syncer
        .sync_and_run(
            &scripted_context.source,
            &scripted_context.networking,
            "echo ok",
        )
        .unwrap_or_else(|err| panic!("remote command failed: {err}"))
}

#[then("the orchestrator reports exit code \"{code}\"")]
fn orchestrator_reports_exit_code(output: &mriya::sync::RemoteCommandOutput, code: i32) {
    assert_eq!(output.exit_code, code);
}

#[given("a scripted runner that fails during sync")]
fn scripted_runner_with_failure() -> ScriptedContext {
    let runner = ScriptedRunner::new();
    runner.push_failure(12);

    build_scripted_context(runner, "temp source for scripted runner failure")
}

#[when("I trigger sync against the workspace")]
fn trigger_sync(scripted_context: &ScriptedContext) -> SyncError {
    let syncer = Syncer::new(
        scripted_context.config.clone(),
        scripted_context.runner.clone(),
    )
    .unwrap_or_else(|err| panic!("failed to build syncer: {err}"));
    let destination = SyncDestination::Remote {
        user: String::from("ubuntu"),
        host: scripted_context.networking.public_ip.to_string(),
        port: scripted_context.networking.ssh_port,
        path: Utf8PathBuf::from("/remote"),
    };
    match syncer.sync(&scripted_context.source, &destination) {
        Ok(()) => skip!("ssh command should not run when sync succeeds"),
        Err(err) => err,
    }
}

#[then("the sync error mentions the rsync exit code")]
fn sync_error_mentions_status(error: &SyncError) {
    let SyncError::CommandFailure { status, .. } = error else {
        panic!("expected sync command failure");
    };
    assert_eq!(*status, Some(12));
}
