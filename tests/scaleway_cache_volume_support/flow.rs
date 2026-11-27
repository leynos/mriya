//! Provider lifecycle helpers for the live three-VM cache suite.
//!
//! These are the real backend, SSH and cache-device calls the suite drives;
//! they live here so the test module keeps the fixture, BDD steps and evidence
//! assertions readable. The guest-side script and sentinel helpers are in the
//! `scripts` submodule.

use anyhow::{Context, Result};
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
    sync::ProcessCommandRunner,
};

use super::{
    harness::{CacheEvidence, CacheHarness, DELETE_POLL, DELETE_WAIT, VOLUME_SIZE_BYTES},
    scripts::{Sentinel, format_volume, mount, read_file, unmount, write_file},
};

/// What one VM does while it holds the cache.
struct Lifetime<F>
where
    F: FnOnce(&InstanceNetworking) -> Result<()>,
{
    /// Whether the new volume must be formatted before it is mounted.
    format: bool,
    /// Action to run while the verified cache is mounted.
    mounted: F,
}

/// Handles shared by the guest-side steps of one lifetime.
struct Guest<'a> {
    /// Live configuration and backend.
    harness: &'a CacheHarness,
    /// SSH runner used for every guest command.
    syncer: &'a Syncer<ProcessCommandRunner>,
    /// Volume whose scripts run on the guest.
    volume: &'a VolumeHandle,
}

impl Guest<'_> {
    /// Formats when required, mounts, runs the lifetime action and unmounts.
    fn exercise<F>(&self, networking: &InstanceNetworking, lifetime: Lifetime<F>) -> Result<()>
    where
        F: FnOnce(&InstanceNetworking) -> Result<()>,
    {
        if lifetime.format {
            format_volume(self.syncer, networking, self.volume)?;
        }
        self.mount(networking)?;
        (lifetime.mounted)(networking)?;
        self.unmount(networking)
    }

    /// Mounts and verifies the cache.
    fn mount(&self, networking: &InstanceNetworking) -> Result<()> {
        mount(self.harness, self.syncer, networking, self.volume)
    }

    /// Flushes and unmounts the cache.
    fn unmount(&self, networking: &InstanceNetworking) -> Result<()> {
        unmount(
            self.syncer,
            networking,
            self.volume,
            &self.harness.sync_config.volume_mount_path,
        )
    }
}

/// Creates, mounts, exercises and tears down one VM, returning its status.
///
/// A guest-side failure still runs an ordered teardown before it is reported,
/// so a broken step cannot leave a paid server running. A detach failure
/// retains the server, because deleting it would destroy the cache.
async fn one_lifetime<F>(
    harness: &CacheHarness,
    syncer: &Syncer<ProcessCommandRunner>,
    volume: &VolumeHandle,
    lifetime: Lifetime<F>,
) -> Result<String>
where
    F: FnOnce(&InstanceNetworking) -> Result<()>,
{
    let (handle, networking) = provision(harness, volume).await?;
    let guest = Guest {
        harness,
        syncer,
        volume,
    };
    let outcome = guest.exercise(&networking, lifetime);
    match outcome {
        Ok(()) => teardown_and_check(harness, handle, volume).await,
        Err(error) => {
            let unmount_note = guest.unmount(&networking).err();
            let teardown = harness.backend.teardown(&handle, Some(&volume.id)).await;
            Err(match teardown {
                Ok(()) => error,
                Err(teardown_error) => anyhow::anyhow!(
                    "{error}; teardown after the failed step also failed: {teardown_error}; server {} may need manual cleanup{}",
                    handle.id,
                    unmount_note.map_or_else(String::new, |note| format!(
                        " (unmount also failed: {note})"
                    ))
                ),
            })
        }
    }
}

/// Sentinel paths and values used across the three lifetimes.
struct Sentinels {
    /// Guest path of sentinel A.
    path_a: String,
    /// Guest path of sentinel B.
    path_b: String,
    /// Value written to sentinel A on the first VM.
    value_a: String,
    /// Value written to sentinel B on the second VM.
    value_b: String,
}

/// Evidence collected while the three VMs run.
struct LifetimeEvidence {
    /// Volume status after each teardown, in order.
    statuses: Vec<String>,
    /// Whether the second VM read sentinel A.
    read_a_on_second: bool,
    /// Whether the third VM read sentinel B.
    read_b_on_third: bool,
}

/// Runs the first VM and writes sentinel A.
async fn run_first(
    harness: &CacheHarness,
    syncer: &Syncer<ProcessCommandRunner>,
    volume: &VolumeHandle,
    sentinels: &Sentinels,
) -> Result<String> {
    let first_sentinel = Sentinel {
        path: &sentinels.path_a,
        value: &sentinels.value_a,
    };
    one_lifetime(
        harness,
        syncer,
        volume,
        Lifetime {
            format: true,
            mounted: |networking: &InstanceNetworking| {
                write_file(
                    syncer,
                    networking,
                    first_sentinel.path,
                    first_sentinel.value,
                )
            },
        },
    )
    .await
}

/// Runs the three VM lifetimes and returns the collected evidence.
async fn run_lifetimes(
    harness: &CacheHarness,
    syncer: &Syncer<ProcessCommandRunner>,
    volume: &VolumeHandle,
    sentinels: &Sentinels,
) -> Result<LifetimeEvidence> {
    let mut statuses = Vec::with_capacity(3);
    statuses.push(run_first(harness, syncer, volume, sentinels).await?);

    let mut read_a_on_second = false;
    let second = one_lifetime(
        harness,
        syncer,
        volume,
        Lifetime {
            format: false,
            mounted: |networking: &InstanceNetworking| {
                read_a_on_second =
                    read_file(syncer, networking, &sentinels.path_a)?.trim() == sentinels.value_a;
                write_file(syncer, networking, &sentinels.path_b, &sentinels.value_b)
            },
        },
    )
    .await?;
    statuses.push(second);

    let mut read_b_on_third = false;
    let third = one_lifetime(
        harness,
        syncer,
        volume,
        Lifetime {
            format: false,
            mounted: |networking: &InstanceNetworking| {
                read_b_on_third =
                    read_file(syncer, networking, &sentinels.path_b)?.trim() == sentinels.value_b;
                Ok(())
            },
        },
    )
    .await?;
    statuses.push(third);

    Ok(LifetimeEvidence {
        statuses,
        read_a_on_second,
        read_b_on_third,
    })
}

/// Runs the full three-VM flow and returns the collected evidence.
pub(super) async fn run_flow(harness: &CacheHarness) -> Result<CacheEvidence> {
    let volume = create_volume(harness).await?;
    match run_with_volume(harness, &volume).await {
        Ok(evidence) => Ok(evidence),
        Err(error) => {
            // A failed run must not leave a billed volume behind. The volume
            // is detached by the lifetime's teardown before this point.
            match harness.backend.delete_volume(&volume).await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(anyhow::anyhow!(
                    "{error}; deleting the test volume {} after the failure also failed: {cleanup}",
                    volume.id
                )),
            }
        }
    }
}

/// Runs the three lifetimes and deletion for an existing volume.
async fn run_with_volume(harness: &CacheHarness, volume: &VolumeHandle) -> Result<CacheEvidence> {
    let syncer =
        Syncer::with_process_runner(harness.sync_config.clone()).context("build syncer")?;
    let mount_path = &harness.sync_config.volume_mount_path;
    let sentinels = Sentinels {
        path_a: format!("{mount_path}/sentinel-a"),
        path_b: format!("{mount_path}/sentinel-b"),
        value_a: format!("sentinel-a-{}", uuid::Uuid::new_v4().simple()),
        value_b: format!("sentinel-b-{}", uuid::Uuid::new_v4().simple()),
    };

    let evidence = run_lifetimes(harness, &syncer, volume, &sentinels).await?;

    harness
        .backend
        .delete_volume(volume)
        .await
        .context("delete cache volume")?;
    let volume_gone = wait_until_gone(harness, volume).await?;

    Ok(CacheEvidence {
        sentinel_a: sentinels.value_a,
        sentinel_b: sentinels.value_b,
        read_a_on_second: evidence.read_a_on_second,
        read_b_on_third: evidence.read_b_on_third,
        volume_statuses: evidence.statuses,
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
    if let Err(error) = harness
        .backend
        .wait_for_volume_status(&volume.zone, &volume.id, VolumeStatus::Available)
        .await
    {
        return match harness.backend.delete_volume(&volume).await {
            Ok(()) => Err(error).context("wait for volume availability"),
            Err(cleanup) => Err(anyhow::anyhow!(
                "waiting for volume {} failed: {error}; deleting it also failed: {cleanup}",
                volume.id
            )),
        };
    }
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
        anyhow::bail!(
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
