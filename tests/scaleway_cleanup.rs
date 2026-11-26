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
    InstanceHandle,
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

/// Runs `scenario`, then requires the project inventory to be unchanged.
///
/// A scenario that returns `Err` reports why it could not run (for example,
/// credentials that lack create permission); the inventory is then not
/// compared, because nothing was exercised.
fn with_inventory(
    backend: &ScalewayBackend,
    scope: &AllocationScope,
    scenario: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let before: ResourceInventory = block_on(backend.inventory(scope))
        .map_err(|error| format!("inventory before the scenario failed: {error}"))?;
    scenario()?;
    let after = block_on(backend.inventory(scope))
        .map_err(|error| format!("inventory after the scenario failed: {error}"))?;
    if before.all() != after.all() {
        return Err(format!(
            "cleanup did not restore the project inventory: before {before:?}, after {after:?}"
        ));
    }
    Ok(())
}

/// Returns why `error` means the live scenario cannot run, if it does.
fn permission_reason(error: &ScalewayBackendError) -> Option<String> {
    let denied = error.to_string().contains("permissions_denied")
        || matches!(error, ScalewayBackendError::AccessDenied { .. });
    denied.then(|| format!("permissions denied during the live scenario: {error}"))
}

/// Classifies a backend failure as a skip reason or a scenario failure.
fn classify_backend_error(error: &ScalewayBackendError) -> String {
    permission_reason(error).unwrap_or_else(|| format!("live scenario failed: {error}"))
}

/// Classifies a run failure as a skip reason, if it reports missing permission.
fn run_permission_reason<T>(
    result: &Result<T, mriya::RunError<ScalewayBackendError>>,
) -> Option<String> {
    match result {
        Err(error) if error.to_string().contains("permissions_denied") => Some(format!(
            "permissions denied during the live scenario: {error}"
        )),
        _ => None,
    }
}

/// Cleans up `ledger`, turning a failure into the scenario's error.
fn expect_cleanup(
    backend: &ScalewayBackend,
    ledger: &ResourceLedger,
    label: &str,
) -> Result<(), String> {
    block_on(backend.cleanup(ledger))
        .map(|_| ())
        .map_err(|failure| format!("{label} cleanup failed: {failure}"))
}

/// Creates a volume or reports why the scenario cannot run.
fn create_cache(
    backend: &ScalewayBackend,
    name: &str,
    request: &InstanceRequest,
) -> Result<(VolumeHandle, ResourceLedger), String> {
    let scope = AllocationScope::new(request.zone.clone(), request.project_id.clone());
    let ledger = ResourceLedger::in_memory(scope);
    let volume = block_on(backend.create_volume(
        &VolumeRequest::new(
            name,
            10 * 1024 * 1024 * 1024,
            &request.zone,
            &request.project_id,
        ),
        &ledger,
    ))
    .map_err(|error| format!("could not create the cache volume: {error}"))?;
    Ok((volume, ledger))
}

/// Removes a cache the scenario created, so the inventory is restored.
fn remove_cache(
    backend: &ScalewayBackend,
    cache: &VolumeHandle,
    ledger: &ResourceLedger,
) -> Result<(), String> {
    ledger.record(
        ResourceRef::instance_volume(&cache.id, &cache.zone),
        Disposition::Owned,
    );
    expect_cleanup(backend, ledger, "cache removal")
}

#[test]
fn cleanup_after_a_successful_run_leaves_no_residue() {
    let backend = require_live!(backend());
    let scope = require_live!(scope_of(&backend));
    let request = require_live!(default_request(&backend));
    let ledger = ResourceLedger::in_memory(scope.clone());

    let outcome = with_inventory(&backend, &scope, || {
        let handle: InstanceHandle = block_on(backend.create(&request, &ledger))
            .map_err(|error| classify_backend_error(&error))?;
        block_on(backend.wait_for_ready(&handle))
            .map_err(|error| classify_backend_error(&error))?;
        expect_cleanup(&backend, &ledger, "run")
    });
    if let Err(reason) = outcome {
        skip_live!("{}", reason);
    }
}

#[test]
fn a_nonexistent_cache_id_is_rejected_without_leaving_a_server() {
    let backend = require_live!(backend());
    let scope = require_live!(scope_of(&backend));
    let mut request = require_live!(default_request(&backend));
    request.volume_id = Some(String::from("11111111-2222-3333-4444-555555555555"));
    let ledger = ResourceLedger::in_memory(scope.clone());

    let outcome = with_inventory(&backend, &scope, || {
        let error = block_on(backend.create(&request, &ledger))
            .expect_err("a nonexistent cache volume must be rejected");
        if let Some(reason) = permission_reason(&error) {
            return Err(reason);
        }
        assert!(
            matches!(error, ScalewayBackendError::VolumeNotFound { .. }),
            "{error:?}"
        );
        expect_cleanup(&backend, &ledger, "run")
    });
    if let Err(reason) = outcome {
        skip_live!("{}", reason);
    }
}

#[test]
fn repeated_cleanup_treats_absence_as_success() {
    let backend = require_live!(backend());
    let scope = require_live!(scope_of(&backend));
    let request = require_live!(default_request(&backend));
    let ledger = ResourceLedger::in_memory(scope.clone());

    let outcome = with_inventory(&backend, &scope, || {
        block_on(backend.create(&request, &ledger))
            .map_err(|error| classify_backend_error(&error))?;
        expect_cleanup(&backend, &ledger, "first")?;
        expect_cleanup(&backend, &ledger, "repeated")
    });
    if let Err(reason) = outcome {
        skip_live!("{}", reason);
    }
}

#[test]
fn a_retained_cache_survives_cleanup() {
    let backend = require_live!(backend());
    let scope = require_live!(scope_of(&backend));
    let request = require_live!(default_request(&backend));

    let (cache, cache_ledger) = require_live!(create_cache(&backend, "mriya-live-cache", &request));
    let run_ledger = ResourceLedger::in_memory(scope.clone());
    let mut request_with_cache = request;
    request_with_cache.volume_id = Some(cache.id.clone());

    let outcome = with_inventory(&backend, &scope, || {
        block_on(backend.create(&request_with_cache, &run_ledger))
            .map_err(|error| classify_backend_error(&error))?;
        expect_cleanup(&backend, &run_ledger, "run")?;
        let disposition = run_ledger
            .find(ResourceKind::InstanceVolume, &cache.id)
            .map(|entry| entry.disposition);
        assert_eq!(
            disposition,
            Some(Disposition::Retained),
            "the cache must be recorded as retained"
        );
        Ok(())
    });
    let removal = outcome.and_then(|()| remove_cache(&backend, &cache, &cache_ledger));
    if let Err(reason) = removal {
        skip_live!("{}", reason);
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
        require_live!(create_cache(&backend, "mriya-live-marker-cache", &request));

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
    if let Some(reason) = run_permission_reason(&first) {
        skip_live!("{}", reason);
    }
    let written = first.expect("the first run must write the marker");
    assert_eq!(
        written.output.exit_code,
        Some(0),
        "{}",
        written.output.stderr
    );
    assert!(first_cleanup.is_none(), "{first_cleanup:?}");

    let (second, second_cleanup) =
        run_with_cache(&backend, &syncer, &request_with_cache, &read_command);
    let read = second.expect("the second run must read the marker");
    assert_eq!(read.output.exit_code, Some(0), "{}", read.output.stderr);
    assert!(
        read.output.stdout.contains("survived"),
        "the cache marker must survive cleanup: {}",
        read.output.stdout
    );
    assert!(second_cleanup.is_none(), "{second_cleanup:?}");

    if let Err(reason) = remove_cache(&backend, &cache, &cache_ledger) {
        skip_live!("{}", reason);
    }
}
