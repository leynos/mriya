//! Opt-in live proof that a cache volume survives successive VMs.
//!
//! The suite is gated like `tests/scaleway_backend.rs`: it runs only when
//! `MRIYA_RUN_SCALEWAY_TESTS` is truthy and live credentials are present. It
//! additionally requires a nonblank `MRIYA_TEST_RUN_ID`, so every server and
//! volume carries a janitor tag and a failed run leaves sweepable resources.
//!
//! The flow writes sentinel A on VM 1, reads A and writes B on VM 2, and reads
//! B on VM 3. After every lifetime it checks that the server is gone and the
//! volume is `available`; it then deletes the volume and checks that it is
//! gone. This is the only test that exercises the real provider for the
//! detach-before-delete contract.

use std::{sync::LazyLock, time::Duration};

use anyhow::{Context, Result, bail};
use mockable::{DefaultEnv, Env};
use mriya::{
    Backend,
    InstanceHandle,
    InstanceNetworking,
    ScalewayBackend,
    ScalewayBackendError,
    ScalewayConfig,
    SyncConfig,
    Syncer,
    VolumeBackend,
    VolumeHandle,
    VolumeRequest,
    VolumeStatus,
    cache_device::{self, CacheDeviceStatus},
    janitor::TEST_RUN_ID_ENV,
    sync::ProcessCommandRunner,
};
use rstest::fixture;
use rstest_bdd::skip;
use rstest_bdd_macros::{given, scenario, then, when};
use shell_escape::unix::escape;
use tokio::runtime::Runtime;

/// Volume size used by the live check: the provider minimum.
const VOLUME_SIZE_BYTES: u64 = 5_000_000_000;
/// How long to wait for a deleted volume to disappear.
const DELETE_WAIT: Duration = Duration::from_secs(120);
/// Delay between deletion checks.
const DELETE_POLL: Duration = Duration::from_secs(2);

static RUNTIME: LazyLock<Result<Runtime, std::io::Error>> = LazyLock::new(Runtime::new);

/// Reports whether the live suite was opted into.
fn integration_enabled() -> bool {
    let enabled = DefaultEnv
        .raw("MRIYA_RUN_SCALEWAY_TESTS")
        .unwrap_or_default();
    let opted_in = matches!(
        enabled.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    );
    let has_run_id = DefaultEnv
        .raw(TEST_RUN_ID_ENV)
        .is_ok_and(|value| !value.trim().is_empty());
    opted_in && has_run_id
}

/// Live configuration and the SSH settings used to drive the guest.
#[derive(Clone)]
struct CacheHarness {
    backend: ScalewayBackend,
    config: ScalewayConfig,
    sync_config: SyncConfig,
}

#[fixture]
fn cache_harness() -> Option<CacheHarness> {
    if !integration_enabled() {
        return None;
    }
    let secret = DefaultEnv.raw("SCW_SECRET_KEY").ok()?;
    let project = DefaultEnv.raw("SCW_DEFAULT_PROJECT_ID").ok()?;
    if secret.trim().is_empty() || project.trim().is_empty() {
        return None;
    }

    let config = ScalewayConfig::load_from_sources().ok()?;
    let backend = ScalewayBackend::new(config.clone()).ok()?;
    let sync_config = SyncConfig::load_without_cli_args().ok()?;
    Some(CacheHarness {
        backend,
        config,
        sync_config,
    })
}

/// Evidence gathered by the three-VM flow.
#[derive(Clone, Debug)]
struct CacheEvidence {
    sentinel_a: String,
    sentinel_b: String,
    read_a_on_second: bool,
    read_b_on_third: bool,
    /// Volume status observed after each VM teardown, in order.
    volume_statuses: Vec<String>,
    volume_gone: bool,
}

/// Failure surfaced by a live step.
#[derive(Debug, thiserror::Error)]
enum StepError {
    /// Human-readable failure description.
    #[error("{0}")]
    Message(String),
}

#[given("live Scaleway credentials and a test run id")]
fn live_credentials(cache_harness: Option<CacheHarness>) {
    if cache_harness.is_none() {
        skip!("live Scaleway cache suite not enabled");
    }
}

/// Runs one async block on the shared Tokio runtime.
fn block_on<T, F>(future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    match RUNTIME.as_ref() {
        Ok(runtime) => runtime.block_on(future),
        Err(error) => Err(anyhow::anyhow!("tokio runtime should start: {error}")),
    }
}

#[when("I run the three-VM cache persistence check")]
fn run_persistence(cache_harness: Option<CacheHarness>) -> Result<CacheEvidence, StepError> {
    let harness = cache_harness
        .ok_or_else(|| StepError::Message(String::from("live cache harness unavailable")))?;
    block_on(run_flow(&harness)).map_err(|error| StepError::Message(format!("{error:#}")))
}

#[then("sentinel A is readable on the second VM")]
fn sentinel_a_read(evidence: &CacheEvidence) {
    assert!(
        evidence.read_a_on_second,
        "VM 2 should read {}",
        evidence.sentinel_a
    );
}

#[then("sentinel B is readable on the third VM")]
fn sentinel_b_read(evidence: &CacheEvidence) {
    assert!(
        evidence.read_b_on_third,
        "VM 3 should read {}",
        evidence.sentinel_b
    );
}

#[then("the volume was available after every VM")]
fn volume_available(evidence: &CacheEvidence) {
    assert_eq!(
        evidence.volume_statuses,
        vec![
            String::from("available"),
            String::from("available"),
            String::from("available")
        ],
        "the volume should be available after each teardown"
    );
}

#[then("the volume is gone after deletion")]
fn volume_gone(evidence: &CacheEvidence) {
    assert!(evidence.volume_gone, "the deleted volume should be gone");
}

#[scenario(
    path = "tests/features/scaleway_cache_volume.feature",
    name = "A cache volume survives three successive VMs"
)]
fn scenario_cache_persistence(cache_harness: Option<CacheHarness>) { drop(cache_harness); }

/// Creates a server with the cache attached and waits for SSH.
async fn provision(
    harness: &CacheHarness,
    volume: &VolumeHandle,
) -> Result<(InstanceHandle, InstanceNetworking)> {
    let mut request = harness.config.as_request().context("build request")?;
    request.volume_id = Some(volume.id.clone());
    request.cloud_init_user_data = None;

    let handle = harness
        .backend
        .create(&request)
        .await
        .context("create instance with cache")?;
    match harness.backend.wait_for_ready(&handle).await {
        Ok(networking) => Ok((handle, networking)),
        Err(error) => {
            if let Err(teardown) = harness.backend.teardown(&handle, Some(&volume.id)).await {
                return Err(anyhow::anyhow!(
                    "readiness failed ({error}) and teardown failed ({teardown}); server {} retained",
                    handle.id
                ));
            }
            Err(error).context("wait for instance readiness")
        }
    }
}

/// Mounts and verifies the cache on `networking`.
fn mount(
    harness: &CacheHarness,
    syncer: &Syncer<ProcessCommandRunner>,
    networking: &InstanceNetworking,
    volume: &VolumeHandle,
) -> Result<()> {
    let script = cache_device::mount_script(
        &volume.id,
        &harness.sync_config.volume_mount_path,
        &harness.sync_config.ssh_user,
    );
    let output = syncer
        .run_remote_raw(networking, &script)
        .context("run mount script")?;
    let report = cache_device::parse_report(&output.stdout).context("parse mount status")?;
    if report.status() != &CacheDeviceStatus::Verified {
        bail!("cache mount was not verified: {}", report.describe());
    }
    Ok(())
}

/// Flushes and unmounts the cache path.
fn unmount(
    syncer: &Syncer<ProcessCommandRunner>,
    networking: &InstanceNetworking,
    mount_path: &str,
) -> Result<()> {
    let script = cache_device::unmount_script(mount_path);
    let output = syncer
        .run_remote_raw(networking, &script)
        .context("run unmount script")?;
    let report = cache_device::parse_report(&output.stdout).context("parse unmount status")?;
    if report.status() != &CacheDeviceStatus::Unmounted {
        bail!("cache unmount failed: {}", report.describe());
    }
    Ok(())
}

/// Writes `contents` to `path` on the guest and flushes it.
fn write_file(
    syncer: &Syncer<ProcessCommandRunner>,
    networking: &InstanceNetworking,
    path: &str,
    contents: &str,
) -> Result<()> {
    let command = format!(
        "printf '%s' {value} | sudo tee {path} >/dev/null && sync",
        value = escape(contents.into()),
        path = escape(path.into()),
    );
    let output = syncer
        .run_remote_raw(networking, &command)
        .context("write sentinel")?;
    if output.exit_code != Some(0) {
        bail!("writing {path} failed with status {:?}", output.exit_code);
    }
    Ok(())
}

/// Reads `path` from the guest and returns its contents.
fn read_file(
    syncer: &Syncer<ProcessCommandRunner>,
    networking: &InstanceNetworking,
    path: &str,
) -> Result<String> {
    let command = format!("sudo cat {path}", path = escape(path.into()));
    let output = syncer
        .run_remote_raw(networking, &command)
        .context("read sentinel")?;
    if output.exit_code != Some(0) {
        bail!("reading {path} failed with status {:?}", output.exit_code);
    }
    Ok(output.stdout)
}

/// Tears the VM down, then confirms the volume is available.
async fn teardown_and_check(
    harness: &CacheHarness,
    handle: InstanceHandle,
    volume: &VolumeHandle,
) -> Result<String> {
    harness
        .backend
        .teardown(&handle, Some(&volume.id))
        .await
        .context("ordered teardown")?;
    let state = harness
        .backend
        .get_volume(&volume.zone, &volume.id)
        .await
        .context("read volume after teardown")?;
    if state.status != VolumeStatus::Available {
        bail!(
            "volume {} should be available after teardown, got {}",
            volume.id,
            state.status
        );
    }
    Ok(state.status.to_string())
}

/// Polls until the deleted volume is no longer found.
async fn wait_until_gone(harness: &CacheHarness, volume: &VolumeHandle) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + DELETE_WAIT;
    while tokio::time::Instant::now() <= deadline {
        match harness.backend.get_volume(&volume.zone, &volume.id).await {
            Err(ScalewayBackendError::VolumeNotInZone { .. }) => return Ok(true),
            Err(error) => return Err(error).context("check deleted volume"),
            Ok(_) => tokio::time::sleep(DELETE_POLL).await,
        }
    }
    Ok(false)
}

/// Creates, mounts and tears down one VM, returning its volume status.
async fn one_lifetime(
    harness: &CacheHarness,
    syncer: &Syncer<ProcessCommandRunner>,
    volume: &VolumeHandle,
    action: impl FnOnce(&InstanceNetworking) -> Result<()>,
) -> Result<String> {
    let (handle, networking) = provision(harness, volume).await?;
    mount(harness, syncer, &networking, volume)?;
    action(&networking)?;
    unmount(syncer, &networking, &harness.sync_config.volume_mount_path)?;
    teardown_and_check(harness, handle, volume).await
}

/// Runs the full three-VM flow and returns the collected evidence.
async fn run_flow(harness: &CacheHarness) -> Result<CacheEvidence> {
    let volume = create_volume(harness).await?;
    let syncer =
        Syncer::with_process_runner(harness.sync_config.clone()).context("build syncer")?;
    let mount_path = harness.sync_config.volume_mount_path.clone();
    let sentinel_a = format!("sentinel-a-{}", uuid::Uuid::new_v4().simple());
    let sentinel_b = format!("sentinel-b-{}", uuid::Uuid::new_v4().simple());
    let path_a = format!("{mount_path}/sentinel-a");
    let path_b = format!("{mount_path}/sentinel-b");
    let mut volume_statuses = Vec::with_capacity(3);

    let first = one_lifetime(harness, &syncer, &volume, |networking| {
        write_file(&syncer, networking, &path_a, &sentinel_a)
    })
    .await?;
    volume_statuses.push(first);

    let mut read_a_on_second = false;
    let second = one_lifetime(harness, &syncer, &volume, |networking| {
        read_a_on_second = read_file(&syncer, networking, &path_a)?.trim() == sentinel_a;
        write_file(&syncer, networking, &path_b, &sentinel_b)
    })
    .await?;
    volume_statuses.push(second);

    let mut read_b_on_third = false;
    let third = one_lifetime(harness, &syncer, &volume, |networking| {
        read_b_on_third = read_file(&syncer, networking, &path_b)?.trim() == sentinel_b;
        Ok(())
    })
    .await?;
    volume_statuses.push(third);

    harness
        .backend
        .delete_volume(&volume)
        .await
        .context("delete cache volume")?;
    let volume_gone = wait_until_gone(harness, &volume).await?;

    Ok(CacheEvidence {
        sentinel_a,
        sentinel_b,
        read_a_on_second,
        read_b_on_third,
        volume_statuses,
        volume_gone,
    })
}

/// Creates a dedicated volume and waits until it is available.
async fn create_volume(harness: &CacheHarness) -> Result<VolumeHandle> {
    let name = format!("mriya-cache-{}", uuid::Uuid::new_v4().simple());
    let request = VolumeRequest::new(
        name,
        VOLUME_SIZE_BYTES,
        harness.config.default_zone.clone(),
        harness.config.default_project_id.clone(),
    );
    let volume = harness
        .backend
        .create_volume(&request)
        .await
        .context("create cache volume")?;
    harness
        .backend
        .wait_for_volume_status(&volume.zone, &volume.id, VolumeStatus::Available)
        .await
        .context("wait for volume availability")?;
    Ok(volume)
}
