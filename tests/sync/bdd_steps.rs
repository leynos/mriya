//! BDD step definitions for sync behaviour, covering workspace caching and
//! remote command exit-code propagation.

use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs_utf8::Dir};
use mriya::sync::{SyncConfig, SyncDestination, SyncError, Syncer};
use rstest_bdd_macros::{given, then, when};

use super::test_doubles::LocalCopyRunner;
use super::test_helpers::{ScriptedContext, Workspace};

#[given("a workspace with a gitignored cache on the remote")]
fn workspace_with_cache(workspace: Workspace) -> Result<Workspace, SyncError> {
    super::test_helpers::write_file(
        workspace.local_root.join(".gitignore").as_path(),
        "target/\n.DS_Store\n",
    )?;
    super::test_helpers::write_file(
        workspace.local_root.join("src").join("lib.rs").as_path(),
        "pub fn meaning() -> u32 { 42 }\n",
    )?;

    super::test_helpers::write_file(
        workspace
            .remote_root
            .join("target")
            .join("cache.txt")
            .as_path(),
        "cached artifact",
    )?;
    super::test_helpers::write_file(
        workspace.remote_root.join("stale.txt").as_path(),
        "remove me",
    )?;

    Ok(workspace)
}

#[when("I run git-aware rsync sync to the remote path")]
fn run_git_aware_sync(workspace: Workspace) -> Result<Workspace, SyncError> {
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

    Ok(workspace)
}

#[then("the gitignored cache directory remains after sync")]
fn cache_survives(workspace: &Workspace) -> Result<(), SyncError> {
    let cache_path = workspace.remote_root.join("target").join("cache.txt");
    if !cache_path.is_file() {
        return Err(SyncError::Spawn {
            program: String::from("rsync"),
            message: String::from("gitignored target directory should be preserved"),
        });
    }
    Ok(())
}

#[then("tracked files are mirrored to the remote")]
fn tracked_files_updated(workspace: &Workspace) -> Result<(), SyncError> {
    let synced_file = workspace.remote_root.join("src").join("lib.rs");
    let fs = Dir::open_ambient_dir("/", ambient_authority()).map_err(|err| SyncError::Spawn {
        program: String::from("fixture"),
        message: err.to_string(),
    })?;
    let relative = synced_file.strip_prefix("/").unwrap_or(&synced_file);
    let contents = fs
        .read_to_string(relative)
        .map_err(|err| SyncError::Spawn {
            program: String::from("fixture"),
            message: err.to_string(),
        })?;
    if !contents.contains("meaning") {
        return Err(SyncError::Spawn {
            program: String::from("rsync"),
            message: String::from("source contents should be synced"),
        });
    }

    if workspace.remote_root.join("stale.txt").exists() {
        return Err(SyncError::Spawn {
            program: String::from("rsync"),
            message: String::from("non-ignored stale files should be removed by rsync --delete"),
        });
    }
    Ok(())
}

#[given("a scripted runner that succeeds at sync")]
fn scripted_runner(scripted_context: ScriptedContext) -> ScriptedContext {
    scripted_context.runner.push_success(); // rsync success
    scripted_context
}

#[when("the remote command exits with \"{code}\"")]
fn remote_command_exits(
    scripted_context: ScriptedContext,
    code: i32,
) -> Result<mriya::sync::RemoteCommandOutput, SyncError> {
    let scripted_context_val = scripted_context;
    scripted_context_val.runner.push_exit_code(code);
    let syncer = Syncer::new(
        scripted_context_val.config.clone(),
        scripted_context_val.runner.clone(),
    )?;
    syncer.sync_and_run(
        &scripted_context_val.source,
        &scripted_context_val.networking,
        "echo ok",
    )
}

#[then("the orchestrator reports exit code \"{code}\"")]
fn orchestrator_reports_exit_code(
    output: &mriya::sync::RemoteCommandOutput,
    code: i32,
) -> Result<(), SyncError> {
    if output.exit_code == Some(code) {
        Ok(())
    } else {
        Err(SyncError::Spawn {
            program: String::from("ssh"),
            message: format!(
                "expected exit code {code}, got {}",
                output
                    .exit_code
                    .map_or_else(|| "None".to_owned(), |value| value.to_string())
            ),
        })
    }
}

#[given("a scripted runner that fails during sync")]
fn scripted_runner_with_failure(scripted_context: ScriptedContext) -> ScriptedContext {
    scripted_context.runner.push_failure(12);
    scripted_context
}

#[when("I trigger sync against the workspace")]
fn trigger_sync(scripted_context: ScriptedContext) -> Result<SyncError, SyncError> {
    let scripted_context_val = scripted_context;
    let syncer = Syncer::new(
        scripted_context_val.config.clone(),
        scripted_context_val.runner.clone(),
    )?;
    let destination = SyncDestination::Remote {
        user: String::from("ubuntu"),
        host: scripted_context_val.networking.public_ip.to_string(),
        port: scripted_context_val.networking.ssh_port,
        path: Utf8PathBuf::from("/remote"),
    };
    let result = syncer.sync(&scripted_context_val.source, &destination);
    match result {
        Ok(()) => Err(SyncError::Spawn {
            program: String::from("rsync"),
            message: String::from("ssh command should not run when sync succeeds"),
        }),
        Err(err) => Ok(err),
    }
}

#[then("the sync error mentions the rsync exit code")]
fn sync_error_mentions_status(error: &SyncError) -> Result<(), SyncError> {
    let SyncError::CommandFailure { status, .. } = error else {
        return Err(SyncError::Spawn {
            program: String::from("rsync"),
            message: String::from("expected sync command failure"),
        });
    };
    if *status == Some(12) {
        Ok(())
    } else {
        Err(SyncError::Spawn {
            program: String::from("rsync"),
            message: format!("expected status 12, got {status:?}"),
        })
    }
}
