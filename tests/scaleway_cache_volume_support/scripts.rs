//! Guest-side cache-device and sentinel-file helpers for the live suite.
//!
//! The scripts are the production ones; this module only runs them over SSH
//! and requires the exact status each must report, so a live run fails at the
//! step whose contract is broken rather than at a later assertion.

use anyhow::{Context, Result, bail};
use mriya::{
    InstanceNetworking,
    Syncer,
    VolumeHandle,
    cache_device::{self, CacheDeviceStatus},
    sync::ProcessCommandRunner,
};
use shell_escape::unix::escape;

use super::harness::CacheHarness;

/// One cache-device script and the status it must report.
#[derive(Clone, Copy)]
enum CacheAction {
    /// The first lifetime formats the new volume.
    Format,
    /// Every lifetime mounts and verifies the cache.
    Mount,
    /// Every lifetime flushes and unmounts the cache.
    Unmount,
}

impl CacheAction {
    /// Returns the label used in failure messages.
    const fn label(self) -> &'static str {
        match self {
            Self::Format => "cache format",
            Self::Mount => "cache mount",
            Self::Unmount => "cache unmount",
        }
    }

    /// Returns the status the script must report.
    const fn expected(self) -> CacheDeviceStatus {
        match self {
            Self::Format => CacheDeviceStatus::Formatted,
            Self::Mount => CacheDeviceStatus::Verified,
            Self::Unmount => CacheDeviceStatus::Unmounted,
        }
    }
}

/// Runs one cache-device script and requires the status it reports.
fn run_script(
    syncer: &Syncer<ProcessCommandRunner>,
    networking: &InstanceNetworking,
    script: &str,
    action: CacheAction,
) -> Result<()> {
    let label = action.label();
    let output = syncer
        .run_remote_raw(networking, script)
        .with_context(|| format!("run {label} script"))?;
    if output.exit_code != Some(0) {
        bail!("{label} exited with status {:?}", output.exit_code);
    }
    let report = cache_device::parse_report(&output.stdout)
        .with_context(|| format!("parse {label} status"))?;
    if report.status() != &action.expected() {
        bail!("{label} failed: {}", report.describe());
    }
    Ok(())
}

/// A sentinel file to write and the value it should contain.
pub(super) struct Sentinel<'a> {
    /// Guest path of the sentinel file.
    pub(super) path: &'a str,
    /// Value written to and read from the file.
    pub(super) value: &'a str,
}

/// Formats the new volume through the guarded cache-device script.
pub(super) fn format_volume(
    syncer: &Syncer<ProcessCommandRunner>,
    networking: &InstanceNetworking,
    volume: &VolumeHandle,
) -> Result<()> {
    run_script(
        syncer,
        networking,
        &cache_device::format_script(&volume.id),
        CacheAction::Format,
    )
}

/// Mounts and verifies the cache on `networking`.
pub(super) fn mount(
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
    run_script(syncer, networking, &script, CacheAction::Mount)
}

/// Flushes and unmounts the cache path.
pub(super) fn unmount(
    syncer: &Syncer<ProcessCommandRunner>,
    networking: &InstanceNetworking,
    volume: &VolumeHandle,
    mount_path: &str,
) -> Result<()> {
    let script = cache_device::unmount_script(&volume.id, mount_path);
    run_script(syncer, networking, &script, CacheAction::Unmount)
}

/// Writes `contents` to `path` on the guest and flushes it.
pub(super) fn write_file(
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
pub(super) fn read_file(
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
