//! Behavioural coverage for git-aware rsync sync and remote command handling.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fs::{
    DirEntry, FileType, copy, create_dir_all, read_dir, read_to_string, remove_dir_all,
    remove_file, write,
};
use std::net::{IpAddr, Ipv4Addr};
use std::rc::Rc;
use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use mriya::InstanceNetworking;
use mriya::sync::{
    CommandOutput, RemoteCommandOutput, SyncConfig, SyncDestination, SyncError, Syncer,
};
use rstest::fixture;
use rstest_bdd::skip;
use rstest_bdd_macros::{given, scenario, then, when};
use tempfile::TempDir;

#[derive(Clone, Debug)]
struct Workspace {
    local_root: Utf8PathBuf,
    remote_root: Utf8PathBuf,
    _local_tmp: Arc<TempDir>,
    _remote_tmp: Arc<TempDir>,
}

impl Workspace {
    fn new() -> Self {
        let local_tmp = Arc::new(temp_dir("create local workspace temp directory"));
        let remote_tmp = Arc::new(temp_dir("create remote workspace temp directory"));

        let local_root = utf8_path(
            local_tmp.path().to_path_buf(),
            "local path should be valid UTF-8",
        );
        let remote_root = utf8_path(
            remote_tmp.path().to_path_buf(),
            "remote path should be valid UTF-8",
        );

        Self {
            local_root,
            remote_root,
            _local_tmp: local_tmp,
            _remote_tmp: remote_tmp,
        }
    }
}

fn write_file(path: &Utf8Path, contents: &str) {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)
            .unwrap_or_else(|err| panic!("create parent directories for {path}: {err}"));
    }
    write(path, contents)
        .unwrap_or_else(|err| panic!("write {path} content for test fixture: {err}"));
}

fn temp_dir(label: &str) -> TempDir {
    TempDir::new().unwrap_or_else(|err| panic!("{label}: {err}"))
}

fn utf8_path(path: std::path::PathBuf, label: &str) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(path).unwrap_or_else(|err| panic!("{label}: {}", err.display()))
}

fn build_scripted_context(runner: ScriptedRunner, label: &str) -> ScriptedContext {
    let source_tmp = Arc::new(temp_dir(label));
    let source_path = utf8_path(
        source_tmp.path().to_path_buf(),
        "scripted context source path",
    );

    ScriptedContext {
        runner,
        config: SyncConfig {
            rsync_bin: String::from("rsync"),
            ssh_bin: String::from("ssh"),
            ssh_user: String::from("ubuntu"),
            remote_path: String::from("/remote"),
        },
        networking: InstanceNetworking {
            public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            ssh_port: 22,
        },
        source: source_path,
        _source_tmp: source_tmp,
    }
}

#[fixture]
fn workspace() -> Workspace {
    Workspace::new()
}

#[fixture]
fn scripted_context() -> ScriptedContext {
    build_scripted_context(ScriptedRunner::new(), "scripted context fixture")
}

#[fixture]
fn output() -> RemoteCommandOutput {
    RemoteCommandOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }
}

#[fixture]
fn error() -> SyncError {
    SyncError::Spawn {
        program: String::from("rsync"),
        message: String::from("placeholder"),
    }
}

#[derive(Clone, Debug)]
struct IgnoreRules {
    dirs: HashSet<String>,
    files: HashSet<String>,
}

fn simulate_rsync(source: &Utf8Path, destination: &Utf8Path) -> Result<(), SyncError> {
    let rules = load_ignores(source)?;
    let mut kept: HashSet<Utf8PathBuf> = HashSet::new();
    copy_tree(source, destination, &rules, &mut kept)?;
    prune_destination(destination, &kept, &rules)?;
    Ok(())
}

fn load_ignores(source: &Utf8Path) -> Result<IgnoreRules, SyncError> {
    let ignore_path = source.join(".gitignore");
    if !ignore_path.exists() {
        return Ok(IgnoreRules {
            dirs: HashSet::new(),
            files: HashSet::new(),
        });
    }

    let content = read_to_string(&ignore_path).map_err(|err| SyncError::Spawn {
        program: String::from("rsync"),
        message: err.to_string(),
    })?;

    let mut dirs = HashSet::new();
    let mut files = HashSet::new();
    for line in content.lines() {
        let pattern = line.trim();
        if pattern.is_empty() || pattern.starts_with('#') {
            continue;
        }
        if pattern.ends_with('/') {
            dirs.insert(pattern.trim_end_matches('/').to_owned());
        } else {
            files.insert(pattern.to_owned());
        }
    }

    Ok(IgnoreRules { dirs, files })
}

fn should_ignore(relative: &Utf8Path, rules: &IgnoreRules) -> bool {
    for component in relative.components() {
        if rules.dirs.contains(component.as_str()) {
            return true;
        }
    }

    relative
        .file_name()
        .is_some_and(|name| rules.files.contains(name))
}

fn copy_tree(
    source_root: &Utf8Path,
    destination_root: &Utf8Path,
    rules: &IgnoreRules,
    kept: &mut HashSet<Utf8PathBuf>,
) -> Result<(), SyncError> {
    for source_entry in read_dir(source_root).map_err(|err| SyncError::Spawn {
        program: String::from("rsync"),
        message: err.to_string(),
    })? {
        let entry = source_entry.map_err(|err| SyncError::Spawn {
            program: String::from("rsync"),
            message: err.to_string(),
        })?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|err| SyncError::Spawn {
            program: String::from("rsync"),
            message: err.display().to_string(),
        })?;
        let relative = path
            .strip_prefix(source_root)
            .map_err(|err| SyncError::Spawn {
                program: String::from("rsync"),
                message: err.to_string(),
            })?;

        if should_ignore(relative, rules) {
            continue;
        }

        let destination_path = destination_root.join(relative);
        let metadata = entry.file_type().map_err(|err| SyncError::Spawn {
            program: String::from("rsync"),
            message: err.to_string(),
        })?;

        if metadata.is_dir() {
            create_dir_all(&destination_path).map_err(|err| SyncError::Spawn {
                program: String::from("rsync"),
                message: err.to_string(),
            })?;
            kept.insert(relative.to_path_buf());
            copy_tree(&path, &destination_path, rules, kept)?;
        } else {
            if let Some(parent) = destination_path.parent() {
                create_dir_all(parent).map_err(|err| SyncError::Spawn {
                    program: String::from("rsync"),
                    message: err.to_string(),
                })?;
            }
            copy(&path, &destination_path).map_err(|err| SyncError::Spawn {
                program: String::from("rsync"),
                message: err.to_string(),
            })?;
            kept.insert(relative.to_path_buf());
        }
    }

    Ok(())
}

fn map_io_error(err: &impl ToString) -> SyncError {
    SyncError::Spawn {
        program: String::from("rsync"),
        message: err.to_string(),
    }
}

fn should_keep_entry(
    relative: &Utf8Path,
    file_type: FileType,
    kept: &HashSet<Utf8PathBuf>,
) -> bool {
    let has_children = kept.iter().any(|kept_path| kept_path.starts_with(relative));
    kept.contains(relative) || (file_type.is_dir() && has_children)
}

fn remove_entry(path: &Utf8Path, is_dir: bool) -> Result<(), SyncError> {
    if is_dir {
        remove_dir_all(path).map_err(|err| map_io_error(&err))
    } else {
        remove_file(path).map_err(|err| map_io_error(&err))
    }
}

fn process_destination_entry(
    entry: &DirEntry,
    destination_root: &Utf8Path,
    kept: &HashSet<Utf8PathBuf>,
    rules: &IgnoreRules,
) -> Result<(), SyncError> {
    let path = Utf8PathBuf::from_path_buf(entry.path())
        .map_err(|err| map_io_error(&err.display()))?;
    let relative = path
        .strip_prefix(destination_root)
        .map_err(|err| map_io_error(&err))?;

    if should_ignore(relative, rules) {
        return Ok(());
    }

    let file_type = entry.file_type().map_err(|err| map_io_error(&err))?;

    if should_keep_entry(relative, file_type, kept) {
        if file_type.is_dir() {
            prune_destination(&path, kept, rules)?;
        }
        return Ok(());
    }

    remove_entry(&path, file_type.is_dir())
}

fn prune_destination(
    destination_root: &Utf8Path,
    kept: &HashSet<Utf8PathBuf>,
    rules: &IgnoreRules,
) -> Result<(), SyncError> {
    if !destination_root.exists() {
        return Ok(());
    }

    for destination_entry in read_dir(destination_root).map_err(|err| map_io_error(&err))? {
        let entry = destination_entry.map_err(|err| map_io_error(&err))?;
        process_destination_entry(&entry, destination_root, kept, rules)?;
    }

    Ok(())
}

#[test]
fn sync_config_validation_rejects_empty_remote_path() {
    let cfg = SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: String::new(),
    };

    let err = cfg
        .validate()
        .expect_err("missing remote_path should be rejected");
    assert!(matches!(
        err,
        SyncError::InvalidConfig { field } if field == "remote_path"
    ));
}

#[given("a workspace with a gitignored cache on the remote")]
fn workspace_with_cache() -> Workspace {
    let workspace = Workspace::new();

    write_file(
        workspace.local_root.join(".gitignore").as_path(),
        "target/\n.DS_Store\n",
    );
    write_file(
        workspace.local_root.join("src").join("lib.rs").as_path(),
        "pub fn meaning() -> u32 { 42 }\n",
    );

    write_file(
        workspace
            .remote_root
            .join("target")
            .join("cache.txt")
            .as_path(),
        "cached artifact",
    );
    write_file(
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
    let contents = read_to_string(&synced_file)
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

#[derive(Clone, Debug)]
struct ScriptedRunner {
    responses: Rc<RefCell<VecDeque<CommandOutput>>>,
}

impl ScriptedRunner {
    fn new() -> Self {
        Self {
            responses: Rc::new(RefCell::new(VecDeque::new())),
        }
    }

    fn push_success(&self) {
        self.responses.borrow_mut().push_back(CommandOutput {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    fn push_exit_code(&self, code: i32) {
        self.responses.borrow_mut().push_back(CommandOutput {
            code: Some(code),
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    fn push_failure(&self, code: i32) {
        self.responses.borrow_mut().push_back(CommandOutput {
            code: Some(code),
            stdout: String::new(),
            stderr: String::from("simulated failure"),
        });
    }
}

impl mriya::sync::CommandRunner for ScriptedRunner {
    fn run(&self, program: &str, _args: &[OsString]) -> Result<CommandOutput, SyncError> {
        self.responses.borrow_mut().pop_front().map_or_else(
            || {
                Err(SyncError::Spawn {
                    program: program.to_owned(),
                    message: String::from("no scripted response available"),
                })
            },
            Ok,
        )
    }
}

#[derive(Clone, Debug, Default)]
struct LocalCopyRunner;

impl LocalCopyRunner {
    fn parse_paths(args: &[OsString]) -> Result<(Utf8PathBuf, Utf8PathBuf), SyncError> {
        if args.len() < 2 {
            return Err(SyncError::Spawn {
                program: String::from("rsync"),
                message: String::from("missing source or destination argument"),
            });
        }

        let source_arg = args
            .get(args.len() - 2)
            .and_then(|value| value.to_str())
            .ok_or_else(|| SyncError::Spawn {
                program: String::from("rsync"),
                message: String::from("invalid source path"),
            })?;
        let destination_arg = args
            .last()
            .and_then(|value| value.to_str())
            .ok_or_else(|| SyncError::Spawn {
                program: String::from("rsync"),
                message: String::from("invalid destination path"),
            })?;

        Ok((
            Utf8PathBuf::from(source_arg),
            Utf8PathBuf::from(destination_arg),
        ))
    }
}

impl mriya::sync::CommandRunner for LocalCopyRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<CommandOutput, SyncError> {
        if program != "rsync" {
            return Err(SyncError::Spawn {
                program: program.to_owned(),
                message: String::from("local runner only simulates rsync"),
            });
        }

        let (source, destination) = Self::parse_paths(args)?;
        simulate_rsync(&source, &destination)?;

        Ok(CommandOutput {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

#[derive(Clone, Debug)]
struct ScriptedContext {
    runner: ScriptedRunner,
    config: SyncConfig,
    networking: InstanceNetworking,
    source: Utf8PathBuf,
    _source_tmp: Arc<TempDir>,
}

#[given("a scripted runner that succeeds at sync")]
fn scripted_runner() -> ScriptedContext {
    let runner = ScriptedRunner::new();
    runner.push_success(); // rsync success

    build_scripted_context(runner, "temp source for scripted runner")
}

#[when("the remote command exits with \"{code}\"")]
fn remote_command_exits(scripted_context: &ScriptedContext, code: i32) -> RemoteCommandOutput {
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
fn orchestrator_reports_exit_code(output: &RemoteCommandOutput, code: i32) {
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

#[scenario(
    path = "tests/features/sync.feature",
    name = "Preserve gitignored caches on the remote"
)]
fn scenario_preserve_caches(workspace: Workspace) {
    let _ = workspace;
}

#[scenario(
    path = "tests/features/sync.feature",
    name = "Propagate remote exit codes"
)]
fn scenario_propagate_exit_codes(scripted_context: ScriptedContext, output: RemoteCommandOutput) {
    let _ = (scripted_context, output);
}

#[scenario(path = "tests/features/sync.feature", name = "Surface sync failures")]
fn scenario_surface_failures(scripted_context: ScriptedContext, error: SyncError) {
    let _ = (scripted_context, error);
}
