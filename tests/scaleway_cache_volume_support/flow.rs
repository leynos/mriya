//! Provider lifecycle helpers for the live three-VM cache suite.
//!
//! These are the real backend, SSH and cache-device calls the suite drives;
//! they live here so the test module keeps the fixture, BDD steps and evidence
//! assertions readable.

use anyhow::{Context, Result, bail};
use mriya::{
    Backend,
    InstanceHandle,
    InstanceNetworking,
    ScalewayBackendError,
    Syncer,
    VolumeBackend,
    VolumeHandle,
    VolumeRequest,
    VolumeStatus,
    cache_device::{self, CacheDeviceStatus},
    sync::ProcessCommandRunner,
};
use shell_escape::unix::escape;

use super::harness::{CacheEvidence, CacheHarness, DELETE_POLL, DELETE_WAIT, VOLUME_SIZE_BYTES};

/// Formats the new volume through the guarded cache-device script.
fn format_volume(
    syncer: &Syncer<ProcessCommandRunner>,
    networking: &InstanceNetworking,
    volume: &VolumeHandle,
) -> Result<()> {
    let script = cache_device::format_script(&volume.id);
    let output = syncer
        .run_remote_raw(networking, &script)
        .context("run format script")?;
    let report = cache_device::parse_report(&output.stdout).context("parse format status")?;
    if report.status() != &CacheDeviceStatus::Formatted {
        bail!("cache format failed: {}", report.describe());
    }
    Ok(())
}

/// A sentinel file to write and the value it should contain.
struct Sentinel<'a> {
    /// Guest path of the sentinel file.
    path: &'a str,
    /// Value written to and read from the file.
    value: &'a str,
}

/// Initializes the volume on the first VM, writes sentinel A and tears down.
async fn first_lifetime(
    harness: &CacheHarness,
    syncer: &Syncer<ProcessCommandRunner>,
    volume: &VolumeHandle,
    sentinel: &Sentinel<'_>,
) -> Result<String> {
    let (handle, networking) = provision(harness, volume).await?;
    format_volume(syncer, &networking, volume)?;
    mount(harness, syncer, &networking, volume)?;
    write_file(syncer, &networking, sentinel.path, sentinel.value)?;
    unmount(
        syncer,
        &networking,
        volume,
        &harness.sync_config.volume_mount_path,
    )?;
    teardown_and_check(harness, handle, volume).await
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
    unmount(
        syncer,
        &networking,
        volume,
        &harness.sync_config.volume_mount_path,
    )?;
    teardown_and_check(harness, handle, volume).await
}

/// Runs the full three-VM flow and returns the collected evidence.
pub(super) async fn run_flow(harness: &CacheHarness) -> Result<CacheEvidence> {
    let volume = create_volume(harness).await?;
    let syncer =
        Syncer::with_process_runner(harness.sync_config.clone()).context("build syncer")?;
    let mount_path = harness.sync_config.volume_mount_path.clone();
    let sentinel_a = format!("sentinel-a-{}", uuid::Uuid::new_v4().simple());
    let sentinel_b = format!("sentinel-b-{}", uuid::Uuid::new_v4().simple());
    let path_a = format!("{mount_path}/sentinel-a");
    let path_b = format!("{mount_path}/sentinel-b");
    let mut volume_statuses = Vec::with_capacity(3);

    let first_sentinel = Sentinel {
        path: &path_a,
        value: &sentinel_a,
    };
    let first = first_lifetime(harness, &syncer, &volume, &first_sentinel).await?;
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
    volume: &VolumeHandle,
    mount_path: &str,
) -> Result<()> {
    let script = cache_device::unmount_script(&volume.id, mount_path);
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
