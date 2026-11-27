//! Live verification that ownership-aware cleanup leaves no residue.
//!
//! Each scenario inventories servers, Instance volumes, Block Storage volumes,
//! and public IPs before and after it runs, then requires the project to be
//! back to its starting state. The suite only runs with
//! `MRIYA_RUN_SCALEWAY_TESTS=1`, against a dedicated project, because it
//! creates and deletes billable resources.
//!
//! These are plain `#[test]` functions, not `rstest-bdd` steps, so a missing
//! opt-in or credential prints why the scenario did not run and returns. It
//! never fails the suite on a host that has no cloud access.

use std::sync::LazyLock;

use mockable::{DefaultEnv, Env};
use mriya::{
    AllocationScope,
    Backend,
    Cancellation,
    CleanupFailure,
    Disposition,
    InstanceRequest,
    ResourceInventory,
    ResourceKind,
    ResourceLedger,
    ResourceRef,
    RunOrchestrator,
    RunOutcome,
    ScalewayBackend,
    ScalewayBackendError,
    ScalewayConfig,
    VolumeBackend,
    VolumeHandle,
    VolumeRequest,
    sync::{ProcessCommandRunner, SyncConfig, Syncer},
};
use rstest::fixture;
use tokio::runtime::Runtime;

static RUNTIME: LazyLock<Result<Runtime, std::io::Error>> = LazyLock::new(Runtime::new);

/// Reports why a live scenario did not run and returns from the test.
macro_rules! skip_live {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        writeln!(std::io::stderr(), "skipping live scenario: {}", format_args!($($arg)*)).ok();
        return;
    }};
}

/// Unwraps a setup result, skipping the scenario on `Err`.
macro_rules! require_live {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(reason) => skip_live!("{}", reason),
        }
    };
}

/// Unwraps cache setup, enforcing a failure and skipping a permitted skip.
macro_rules! require_cache_live {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(outcome) => {
                outcome.enforce("cache setup");
                return;
            }
        }
    };
}

/// Reports whether the live suite was opted into.
fn enabled() -> bool {
    matches!(
        DefaultEnv
            .raw("MRIYA_RUN_SCALEWAY_TESTS")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// Builds a live backend, or the reason the scenario must skip.
#[fixture]
fn backend() -> Result<ScalewayBackend, String> {
    if !enabled() {
        return Err(String::from(
            "Scaleway integration tests disabled (set MRIYA_RUN_SCALEWAY_TESTS=1 to enable)",
        ));
    }
    let secret = DefaultEnv
        .raw("SCW_SECRET_KEY")
        .map_err(|error| format!("missing SCW_SECRET_KEY: {error}"))?;
    if secret.trim().is_empty() {
        return Err(String::from("empty SCW_SECRET_KEY"));
    }
    let config = ScalewayConfig::load_without_cli_args()
        .map_err(|error| format!("failed to load config: {error}"))?;
    ScalewayBackend::new(config).map_err(|error| format!("failed to build backend: {error}"))
}

/// Builds a syncer for the configured SSH identity, or the skip reason.
fn live_syncer() -> Result<Syncer<ProcessCommandRunner>, String> {
    let identity = DefaultEnv
        .raw("MRIYA_SYNC_SSH_IDENTITY_FILE")
        .map_err(|error| format!("missing MRIYA_SYNC_SSH_IDENTITY_FILE: {error}"))?;
    if identity.trim().is_empty() {
        return Err(String::from("empty MRIYA_SYNC_SSH_IDENTITY_FILE"));
    }
    let sync_config = SyncConfig::load_without_cli_args()
        .map_err(|error| format!("failed to load sync config: {error}"))?;
    Syncer::new(sync_config, ProcessCommandRunner)
        .map_err(|error| format!("syncer init failed: {error}"))
}

/// Runs `future` on the shared test runtime.
fn block_on<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    RUNTIME
        .as_ref()
        .expect("the test runtime must start")
        .block_on(future)
}

/// Returns the allocation scope of the configured defaults, or the skip reason.
fn scope_of(backend: &ScalewayBackend) -> Result<AllocationScope, String> {
    let request = default_request(backend)?;
    Ok(AllocationScope::new(request.zone, request.project_id))
}

/// Returns the configured default instance request, or the skip reason.
fn default_request(backend: &ScalewayBackend) -> Result<InstanceRequest, String> {
    backend
        .default_request()
        .map_err(|error| format!("default request unavailable: {error}"))
}

/// Why a live scenario could not run, or the residue it detected.
///
/// The two are deliberately distinct: an unavailable prerequisite (no opt-in,
/// no credentials, a token without create permission) skips the scenario, while
/// anything the scenario exercised and then failed to clean up fails it.
enum LiveOutcome {
    /// The scenario completed and the inventory was restored.
    Passed,
    /// A prerequisite was unavailable, so nothing was exercised.
    Skip(String),
    /// The scenario ran and detected a defect.
    Failed(String),
}

impl LiveOutcome {
    /// Reports a skip or fails the test accordingly.
    fn enforce(self, scenario: &str) {
        match self {
            Self::Passed => {}
            Self::Skip(reason) => skip_live!("{scenario}: {reason}"),
            Self::Failed(reason) => panic!("{scenario}: {reason}"),
        }
    }
}

/// Runs `scenario`, then requires the project inventory to be unchanged.
fn with_inventory(
    backend: &ScalewayBackend,
    scope: &AllocationScope,
    scenario: impl FnOnce() -> Result<(), LiveOutcome>,
) -> LiveOutcome {
    let before: ResourceInventory = match block_on(backend.inventory(scope)) {
        Ok(before) => before,
        Err(error) => return LiveOutcome::Failed(format!("inventory before the run: {error}")),
    };
    match scenario() {
        Ok(()) => {}
        Err(outcome) => return outcome,
    }
    let after = match block_on(backend.inventory(scope)) {
        Ok(after) => after,
        Err(error) => return LiveOutcome::Failed(format!("inventory after the run: {error}")),
    };
    if before.all() == after.all() {
        return LiveOutcome::Passed;
    }
    LiveOutcome::Failed(format!(
        "cleanup did not restore the project inventory: before {before:?}, after {after:?}"
    ))
}

/// Returns why `error` means the live scenario cannot run, if it does.
fn permission_reason(error: &ScalewayBackendError) -> Option<LiveOutcome> {
    let denied = error.to_string().contains("permissions_denied")
        || matches!(error, ScalewayBackendError::AccessDenied { .. });
    denied.then(|| LiveOutcome::Skip(format!("permissions denied: {error}")))
}

/// Classifies a backend failure as a skip reason or a scenario failure.
fn classify_backend_error(error: &ScalewayBackendError) -> LiveOutcome {
    permission_reason(error)
        .unwrap_or_else(|| LiveOutcome::Failed(format!("backend failure: {error}")))
}

/// Classifies a run failure as a skip reason, if it reports missing permission.
fn run_permission_reason(
    result: &Result<RunOutcome, mriya::RunError<ScalewayBackendError>>,
) -> Option<LiveOutcome> {
    match result {
        Err(error) if error.to_string().contains("permissions_denied") => {
            Some(LiveOutcome::Skip(format!("permissions denied: {error}")))
        }
        _ => None,
    }
}

/// Cleans up `ledger`, turning a failure into the scenario's outcome.
fn expect_cleanup(
    backend: &ScalewayBackend,
    ledger: &ResourceLedger,
    label: &str,
) -> Result<(), LiveOutcome> {
    block_on(backend.cleanup(ledger))
        .map(|_| ())
        .map_err(|failure| LiveOutcome::Failed(format!("{label} cleanup failed: {failure}")))
}

/// Removes a cache on every path, including after a failed assertion.
fn always_remove_cache(
    backend: &ScalewayBackend,
    cache: &VolumeHandle,
    ledger: &ResourceLedger,
) -> Result<(), String> {
    ledger.record(
        ResourceRef::instance_volume(&cache.id, &cache.zone),
        Disposition::Owned,
    );
    block_on(backend.cleanup(ledger))
        .map(|_| ())
        .map_err(|failure| format!("cache removal failed: {failure}"))
}

/// Creates a volume or reports why the scenario cannot run.
fn create_cache(
    backend: &ScalewayBackend,
    name: &str,
    request: &InstanceRequest,
) -> Result<(VolumeHandle, ResourceLedger), LiveOutcome> {
    let scope = AllocationScope::new(request.zone.clone(), request.project_id.clone());
    let ledger = ResourceLedger::in_memory(scope);
    match block_on(backend.create_volume(
        &VolumeRequest::new(
            name,
            10 * 1024 * 1024 * 1024,
            &request.zone,
            &request.project_id,
        ),
        &ledger,
    )) {
        Ok(volume) => Ok((volume, ledger)),
        Err(error) => {
            // The backend contract records allocations on the error path, so
            // the in-memory ledger must be cleaned here or a created volume is
            // left billable with no journal to recover it.
            let cleanup = block_on(backend.cleanup(&ledger));
            match cleanup {
                Ok(_) => Err(permission_reason(&error).unwrap_or_else(|| {
                    LiveOutcome::Failed(format!("could not create the cache volume: {error}"))
                })),
                Err(failure) => Err(LiveOutcome::Failed(format!(
                    "could not create the cache volume: {error}; cleanup also failed: {failure}"
                ))),
            }
        }
    }
}

#[test]
fn cleanup_after_a_successful_run_leaves_no_residue() {
    let backend = require_live!(backend());
    let scope = require_live!(scope_of(&backend));
    let request = require_live!(default_request(&backend));
    let ledger = ResourceLedger::in_memory(scope.clone());

    let outcome = with_inventory(&backend, &scope, || {
        let created = block_on(backend.create(&request, &ledger))
            .map_err(|error| classify_backend_error(&error));
        let handle = match created {
            Ok(handle) => handle,
            Err(outcome) => return Err(outcome),
        };
        // A readiness failure must still be followed by cleanup; propagating
        // it with `?` would leave the server allocated.
        let ready = block_on(backend.wait_for_ready(&handle));
        let cleanup = expect_cleanup(&backend, &ledger, "run");
        match (ready, cleanup) {
            (_, Err(outcome)) => Err(outcome),
            (Err(error), Ok(())) => Err(classify_backend_error(&error)),
            (Ok(_), Ok(())) => Ok(()),
        }
    });
    outcome.enforce("successful run");
}

#[test]
fn a_nonexistent_cache_id_is_rejected_without_leaving_a_server() {
    let backend = require_live!(backend());
    let scope = require_live!(scope_of(&backend));
    let mut request = require_live!(default_request(&backend));
    request.volume_id = Some(String::from("11111111-2222-3333-4444-555555555555"));
    let ledger = ResourceLedger::in_memory(scope.clone());

    let outcome = with_inventory(&backend, &scope, || {
        let created = block_on(backend.create(&request, &ledger));
        // Cleanup runs before any assertion: a create that unexpectedly
        // succeeded must not leave its server allocated.
        let cleanup = expect_cleanup(&backend, &ledger, "run");
        let Err(error) = created else {
            return Err(LiveOutcome::Failed(String::from(
                "a nonexistent cache volume must be rejected",
            )));
        };
        if let Some(outcome) = permission_reason(&error) {
            return Err(outcome);
        }
        if !matches!(error, ScalewayBackendError::VolumeNotFound { .. }) {
            return Err(LiveOutcome::Failed(format!(
                "expected VolumeNotFound, got {error:?}"
            )));
        }
        cleanup
    });
    outcome.enforce("nonexistent cache id");
}

#[test]
fn repeated_cleanup_treats_absence_as_success() {
    let backend = require_live!(backend());
    let scope = require_live!(scope_of(&backend));
    let request = require_live!(default_request(&backend));
    let ledger = ResourceLedger::in_memory(scope.clone());

    let outcome = with_inventory(&backend, &scope, || {
        let created = block_on(backend.create(&request, &ledger))
            .map_err(|error| classify_backend_error(&error));
        // A create that records a server and then fails must still be cleaned;
        // propagating the error with `?` would leave the server allocated.
        let first = expect_cleanup(&backend, &ledger, "first");
        match created {
            Ok(_handle) => {}
            Err(outcome) => return Err(outcome),
        }
        first?;
        expect_cleanup(&backend, &ledger, "repeated")
    });
    outcome.enforce("repeated cleanup");
}

#[test]
fn a_retained_cache_survives_cleanup() {
    let backend = require_live!(backend());
    let scope = require_live!(scope_of(&backend));
    let request = require_live!(default_request(&backend));

    let (cache, cache_ledger) =
        require_cache_live!(create_cache(&backend, "mriya-live-cache", &request));
    let run_ledger = ResourceLedger::in_memory(scope.clone());
    let mut request_with_cache = request;
    request_with_cache.volume_id = Some(cache.id.clone());

    let outcome = with_inventory(&backend, &scope, || {
        let created = block_on(backend.create(&request_with_cache, &run_ledger))
            .map_err(|error| classify_backend_error(&error));
        // Cleanup runs before the result is inspected, so a create failure
        // cannot strand the server it may have recorded.
        let cleanup = expect_cleanup(&backend, &run_ledger, "run");
        let Ok(_handle) = created else {
            return Err(created.err().unwrap_or(LiveOutcome::Passed));
        };
        cleanup?;
        let disposition = run_ledger
            .find(ResourceKind::InstanceVolume, &cache.id)
            .map(|entry| entry.disposition);
        if disposition != Some(Disposition::Retained) {
            return Err(LiveOutcome::Failed(format!(
                "the cache must be recorded as retained, got {disposition:?}"
            )));
        }
        Ok(())
    });
    // The cache outlives the scenario, so it is removed on every path.
    let removal = always_remove_cache(&backend, &cache, &cache_ledger);
    match (outcome, removal) {
        (LiveOutcome::Failed(reason), _) | (_, Err(reason)) => {
            LiveOutcome::Failed(reason).enforce("retained cache");
        }
        (LiveOutcome::Skip(reason), Ok(())) => LiveOutcome::Skip(reason).enforce("retained cache"),
        (LiveOutcome::Passed, Ok(())) => {}
    }
}

/// A live run that executes `command` with the cache attached, then cleans up.
fn run_with_cache(
    backend: &ScalewayBackend,
    syncer: &Syncer<ProcessCommandRunner>,
    request: &InstanceRequest,
    command: &str,
) -> (
    Result<RunOutcome, mriya::RunError<ScalewayBackendError>>,
    Option<CleanupFailure>,
) {
    let ledger = ResourceLedger::in_memory(AllocationScope::new(
        request.zone.clone(),
        request.project_id.clone(),
    ));
    let orchestrator: RunOrchestrator<ScalewayBackend, ProcessCommandRunner> =
        RunOrchestrator::new(backend.clone(), syncer.clone());
    let source = camino::Utf8PathBuf::from(".");
    let cancellation = Cancellation::never();
    let result = block_on(orchestrator.execute(mriya::run::RunInputs {
        request,
        ledger: &ledger,
        source: &source,
        remote_command: command,
        cancellation: &cancellation,
    }));
    let cleanup = block_on(backend.cleanup(&ledger)).err();
    (result, cleanup)
}

#[test]
fn a_retained_cache_keeps_its_marker_across_runs() {
    let backend = require_live!(backend());
    let syncer = require_live!(live_syncer());
    let request = require_live!(default_request(&backend));

    let (cache, cache_ledger) =
        require_cache_live!(create_cache(&backend, "mriya-live-marker-cache", &request));

    // `mriya init` formats the volume; this scenario does it inside the first
    // run so it can also write the marker.
    let device = format!("/dev/disk/by-id/scsi-0SCW_BSSD_{id}", id = cache.id);
    let write_command = format!(
        "sudo mkfs.ext4 -F {device} && sudo mount {device} /mriya && \
         printf mriya-cache-marker | sudo tee /mriya/mriya-marker >/dev/null"
    );
    let read_command =
        String::from("cat /mriya/mriya-marker | grep -q mriya-cache-marker && echo survived");

    let mut request_with_cache = request;
    request_with_cache.volume_id = Some(cache.id.clone());

    let (first, first_cleanup) =
        run_with_cache(&backend, &syncer, &request_with_cache, &write_command);
    if let Some(outcome) = run_permission_reason(&first) {
        let removal = always_remove_cache(&backend, &cache, &cache_ledger);
        // A cache this test created must be removed even when the scenario
        // cannot run; a failed removal is a real residue, not a skip.
        match removal {
            Ok(()) => outcome.enforce("marker write"),
            Err(reason) => LiveOutcome::Failed(reason).enforce("marker write"),
        }
        return;
    }

    // Both runs are captured before anything panics: an assertion failure must
    // not strand the cache this test created.
    let (second, second_cleanup) = match first.as_ref() {
        Ok(written) if written.output.exit_code == Some(0) => {
            run_with_cache(&backend, &syncer, &request_with_cache, &read_command)
        }
        _ => (
            Err(mriya::RunError::Provision(ScalewayBackendError::Provider {
                message: String::from("the first run did not complete"),
            })),
            None,
        ),
    };
    let removal = always_remove_cache(&backend, &cache, &cache_ledger);

    let written = first.expect("the first run must write the marker");
    assert_eq!(
        written.output.exit_code,
        Some(0),
        "{}",
        written.output.stderr
    );
    assert!(first_cleanup.is_none(), "{first_cleanup:?}");

    let read = second.expect("the second run must read the marker");
    assert_eq!(read.output.exit_code, Some(0), "{}", read.output.stderr);
    assert!(
        read.output.stdout.contains("survived"),
        "the cache marker must survive cleanup: {}",
        read.output.stdout
    );
    assert!(second_cleanup.is_none(), "{second_cleanup:?}");

    if let Err(reason) = removal {
        LiveOutcome::Failed(reason).enforce("marker cache removal");
    }
}
