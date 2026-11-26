//! Behavioural tests for the benchmark observation seam.
//!
//! The seam's contract is asymmetric on purpose: an observer may see what
//! happened, and a run must not change because one is attached. These tests
//! drive `RunOrchestrator` through the real syncer with a scripted runner and a
//! scripted backend, so they assert the phases the orchestrator actually
//! reports rather than a mock's idea of them.

use std::{
    collections::VecDeque,
    ffi::OsString,
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
};

use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs_utf8::Dir};
use mriya::{
    Backend,
    InstanceHandle,
    InstanceNetworking,
    InstanceRequest,
    InstanceRequestBuilder,
    RunOrchestrator,
    SyncConfig,
    backend::BackendFuture,
    bench::{
        observe::{Observation, RecordedObservation, RecordingObserver, RunObserver},
        record::PhaseKind,
    },
    sync::{CommandOutput, CommandRunner, SyncError, Syncer},
};
use tempfile::TempDir;
use thiserror::Error;

/// Errors a scripted backend can produce.
#[derive(Clone, Debug, Error)]
enum ScriptedError {
    /// Raised when the scenario asked teardown to fail.
    #[error("scripted teardown failure")]
    Teardown,
}

/// A backend that succeeds at everything, for the happy-path scenarios.
#[derive(Clone, Copy, Debug, Default)]
struct ScriptedBackend {
    /// Whether teardown should fail.
    fail_teardown: bool,
}

impl Backend for ScriptedBackend {
    type Error = ScriptedError;

    fn create<'a>(
        &'a self,
        _request: &'a InstanceRequest,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        Box::pin(async {
            Ok(InstanceHandle {
                id: String::from("srv-1"),
                zone: String::from("zone"),
            })
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        Box::pin(async {
            Ok(InstanceNetworking {
                public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                ssh_port: 22,
            })
        })
    }

    fn destroy(&self, _handle: InstanceHandle) -> BackendFuture<'_, (), Self::Error> {
        let fail = self.fail_teardown;
        Box::pin(async move {
            if fail {
                Err(ScriptedError::Teardown)
            } else {
                Ok(())
            }
        })
    }
}

/// A command runner that returns scripted exit codes in order.
#[derive(Clone, Debug, Default)]
struct ScriptedRunner {
    /// Exit codes to hand out, oldest first.
    codes: Arc<Mutex<VecDeque<i32>>>,
}

impl ScriptedRunner {
    /// Creates a runner that returns `codes` in order.
    fn new(codes: impl IntoIterator<Item = i32>) -> Self {
        Self {
            codes: Arc::new(Mutex::new(codes.into_iter().collect())),
        }
    }
}

impl CommandRunner for ScriptedRunner {
    fn run(&self, _program: &str, _args: &[OsString]) -> Result<CommandOutput, SyncError> {
        let code = self
            .codes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        Ok(CommandOutput {
            code,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// A temporary workspace and the configuration a run needs.
struct Fixture {
    /// The directory synced to the remote.
    source: Utf8PathBuf,
    /// Keeps the temporary directory alive for the test's lifetime.
    _tmp: TempDir,
    /// The sync configuration under test.
    config: SyncConfig,
}

/// Builds a workspace containing one file, with a sync configuration that
/// reaches the scripted runner rather than a real host.
fn fixture() -> Fixture {
    let tmp = TempDir::new().expect("a temporary directory");
    let temporary =
        Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("a UTF-8 temporary path");
    let directory = Dir::open_ambient_dir(&temporary, ambient_authority())
        .expect("the temporary directory should open");
    directory
        .write("README.md", "hello\n")
        .expect("the workspace file should write");
    let source =
        Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("a UTF-8 temporary path");
    let config = SyncConfig {
        rsync_bin: String::from("rsync-stub"),
        ssh_bin: String::from("ssh-stub"),
        ssh_user: String::from("root"),
        remote_path: String::from("/tmp/project"),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
        ssh_identity_file: None,
        volume_mount_path: String::from("/mriya"),
        route_build_caches: true,
        create_cache_directories: true,
    };
    Fixture {
        source,
        _tmp: tmp,
        config,
    }
}

/// Builds the instance request the scenarios use.
fn request(volume_id: Option<&str>) -> InstanceRequest {
    InstanceRequestBuilder::new()
        .image_label("ubuntu")
        .instance_type("DEV1-S")
        .zone("fr-par-1")
        .project_id("project")
        .architecture("x86_64")
        .volume_id(volume_id.map(str::to_owned))
        .build()
        .expect("a valid instance request")
}

/// Adapts an `Arc<RecordingObserver>` to the boxed observer the orchestrator
/// takes, so a test keeps its own handle to the recorded observations.
struct SharedObserver(Arc<RecordingObserver>);

impl RunObserver for SharedObserver {
    fn observe(&self, observation: Observation<'_>) { self.0.observe(observation); }
}

/// Runs one attempt and returns its outcome with the observations it produced.
fn run_once(
    fixture: &Fixture,
    backend: ScriptedBackend,
    codes: impl IntoIterator<Item = i32>,
    volume_id: Option<&str>,
) -> (Result<(), String>, Arc<RecordingObserver>) {
    let syncer =
        Syncer::new(fixture.config.clone(), ScriptedRunner::new(codes)).expect("a valid syncer");
    let observer = Arc::new(RecordingObserver::default());
    let orchestrator = RunOrchestrator::new(backend, syncer)
        .with_observer(Box::new(SharedObserver(observer.clone())));

    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let result = runtime.block_on(async {
        orchestrator
            .execute(&request(volume_id), &fixture.source, "cargo test")
            .await
    });
    let summary = result.map(|_| ()).map_err(|err| err.to_string());
    (summary, observer)
}

#[test]
fn a_complete_run_reports_every_phase_in_order() {
    let fixture = fixture();
    // rsync and the remote command each consume a scripted status.
    let (result, observer) = run_once(&fixture, ScriptedBackend::default(), [0, 0], None);
    result.expect("the run should succeed");

    assert_eq!(
        observer.phases(),
        vec![
            PhaseKind::Allocate,
            PhaseKind::Bootstrap,
            PhaseKind::Sync,
            PhaseKind::RemoteExecution,
            PhaseKind::Teardown,
        ],
        "each phase is reported exactly once, in execution order"
    );
}

#[test]
fn a_mounted_cache_is_reported_as_the_cache_restore_phase() {
    let fixture = fixture();
    // The mount call, rsync, and the remote command each consume a status.
    let (result, observer) = run_once(
        &fixture,
        ScriptedBackend::default(),
        [0, 0, 0],
        Some("vol-1"),
    );
    result.expect("the run should succeed");

    assert!(
        observer.phases().contains(&PhaseKind::CacheRestore),
        "a mounted volume is its own phase: {:?}",
        observer.phases()
    );
}

#[test]
fn a_failed_teardown_is_reported_as_a_failed_phase() {
    let fixture = fixture();
    let (result, observer) = run_once(
        &fixture,
        ScriptedBackend {
            fail_teardown: true,
        },
        [0, 0],
        None,
    );
    assert!(result.is_err(), "the teardown failure must surface");

    let finished = observer
        .observations()
        .into_iter()
        .filter_map(|observation| match observation {
            RecordedObservation::Finished {
                phase, succeeded, ..
            } => Some((phase, succeeded)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        finished.contains(&(PhaseKind::Teardown, false)),
        "a failed teardown is reported as failed, not omitted: {finished:?}"
    );
}

#[test]
fn the_default_observer_changes_nothing_about_the_result() {
    let fixture = fixture();
    // The rsync call and the remote command each consume a scripted status.
    let syncer =
        Syncer::new(fixture.config.clone(), ScriptedRunner::new([0, 0])).expect("a syncer");
    let orchestrator = RunOrchestrator::new(ScriptedBackend::default(), syncer);

    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let result = runtime
        .block_on(async {
            orchestrator
                .execute(&request(None), &fixture.source, "cargo test")
                .await
        })
        .expect("the run should succeed");
    assert_eq!(result.exit_code, Some(0));
}
