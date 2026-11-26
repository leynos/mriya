//! Post-provisioning steps for the run workflow.
//!
//! Mounting, synchronization, remote execution and ordered teardown all need
//! the instance handle plus, once SSH is reachable, its networking details.
//! They live here to keep the orchestration module readable.

use std::{fmt::Display, time::Instant};

use camino::Utf8Path;
use shell_escape::unix::escape;
use tokio::time::sleep;

use super::{ProvisionedInstance, RunError, RunOrchestrator, StepContext};
use crate::{
    backend::{InstanceNetworking, InstanceRequest},
    cache_device::{self, CacheDeviceStatus},
    sync::{CommandRunner, RemoteCommandOutput, SyncDestination, create_cache_directories_command},
    teardown::append_teardown_note,
    volume::VolumeBackend,
};

impl<B, R> RunOrchestrator<B, R>
where
    B: crate::backend::Backend + VolumeBackend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
{
    /// Mounts the cache volume only when the request attached one.
    ///
    /// The instance context is the single source of the cache volume ID: the
    /// request must agree with it, so mount and teardown can never read
    /// different values. A request without an attached volume is the unchanged
    /// no-cache path.
    pub(super) async fn mount_volume_if_needed(
        &self,
        context: StepContext<'_>,
        request: &InstanceRequest,
    ) -> Result<(), RunError<B::Error>> {
        let Some(volume_id) = context.instance.volume_id else {
            return Ok(());
        };
        if request.volume_id.as_deref() != Some(volume_id) {
            return Err(RunError::CacheMount {
                message: String::from(
                    "internal error: the configured volume and the instance context disagree",
                ),
                source: None,
            });
        }
        self.mount_cache_volume(context.instance, context.networking, volume_id)
            .await
    }

    /// Syncs the workspace to `dest`, tearing the instance down if that fails.
    ///
    /// For example, a failed rsync returns the sync error with the teardown
    /// outcome appended, so a leaked instance is never reported as a plain
    /// sync failure.
    pub(super) async fn sync_or_destroy(
        &self,
        context: StepContext<'_>,
        source: &Utf8Path,
        dest: &SyncDestination,
    ) -> Result<(), RunError<B::Error>> {
        if let Err(err) = self.syncer.sync(source, dest) {
            let message = self
                .teardown_with_note(context.instance, Some(context.networking), &err)
                .await;
            return Err(RunError::Sync {
                message,
                source: err,
            });
        }
        Ok(())
    }

    /// Runs `remote_command`, tearing the instance down only if it could not be
    /// started.
    ///
    /// A non-zero exit code from the command is a successful result here.
    ///
    /// Cache routing is always allowed at this point: a configured cache that
    /// did not verify has already failed the run, and a run without one keeps
    /// the guarded preamble.
    ///
    /// For example, a command exiting 7 returns that status in
    /// [`RemoteCommandOutput`], while a failure to start the command destroys
    /// the instance and returns the error text.
    pub(super) async fn run_remote_or_destroy(
        &self,
        context: StepContext<'_>,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, RunError<B::Error>> {
        match self
            .syncer
            .run_remote_with_cache(context.networking, remote_command, true)
        {
            Ok(result) => Ok(result),
            Err(err) => {
                let message = self
                    .teardown_with_note(context.instance, Some(context.networking), &err)
                    .await;
                Err(RunError::Remote {
                    message,
                    source: err,
                })
            }
        }
    }

    /// Mounts the configured cache volume and verifies the mount source.
    ///
    /// The script resolves the disk by its `volume-<uuid>` serial, refuses the
    /// root disk, requires an ext4 signature, refuses a different source at the
    /// path, mounts the device and confirms with `findmnt` that the path's
    /// source is that device. Any other outcome fails the run and tears the
    /// instance down: a configured cache is required, not best-effort.
    ///
    /// When `create_cache_directories` is enabled, the cache subdirectories are
    /// created after verification so language toolchains can write
    /// immediately. The `mountpoint -q` guard remains as a second check.
    pub(super) async fn mount_cache_volume(
        &self,
        instance: ProvisionedInstance<'_>,
        networking: &InstanceNetworking,
        volume_id: &str,
    ) -> Result<(), RunError<B::Error>> {
        let config = self.syncer.config();
        let mount_path = config.volume_mount_path.clone();
        let script = cache_device::mount_script(volume_id, &mount_path, &config.ssh_user);

        let output = match self.syncer.run_remote_raw(networking, &script) {
            Ok(output) => output,
            Err(err) => {
                let message = self
                    .teardown_with_note(instance, Some(networking), &err)
                    .await;
                return Err(RunError::CacheMount {
                    message,
                    source: Some(err),
                });
            }
        };

        let report = match cache_device::parse_report(&output.stdout) {
            Ok(report) => report,
            Err(error) => {
                let failure = format!("cache mount script produced no usable status: {error}");
                let message = self
                    .teardown_with_note(instance, Some(networking), &failure)
                    .await;
                return Err(RunError::CacheMount {
                    message,
                    source: None,
                });
            }
        };

        if report.status() != &CacheDeviceStatus::Verified {
            let failure = format!(
                "cache volume {volume_id} was not verified: {}",
                report.describe()
            );
            let message = self
                .teardown_with_note(instance, Some(networking), &failure)
                .await;
            return Err(RunError::CacheMount {
                message,
                source: None,
            });
        }

        if config.create_cache_directories {
            let command = format!(
                "if mountpoint -q {path} 2>/dev/null; then {create}; fi",
                path = escape(mount_path.as_str().into()),
                create = create_cache_directories_command(&mount_path)
            );
            if let Err(err) = self.syncer.run_remote_raw(networking, &command) {
                let message = self
                    .teardown_with_note(instance, Some(networking), &err)
                    .await;
                return Err(RunError::CacheMount {
                    message,
                    source: Some(err),
                });
            }
        }

        Ok(())
    }

    /// Polls for cloud-init's `boot-finished` marker until it appears or the
    /// timeout elapses.
    ///
    /// A failure to run the check, or a timeout, destroys the instance.
    ///
    /// For example, the marker `/var/lib/cloud/instance/boot-finished`
    /// appearing ends the wait successfully, while the deadline passing
    /// destroys the instance and reports the timeout.
    pub(super) async fn wait_for_cloud_init(
        &self,
        context: StepContext<'_>,
    ) -> Result<(), RunError<B::Error>> {
        let deadline = Instant::now() + self.cloud_init_wait_timeout;
        let cloud_init_finished_marker = "/var/lib/cloud/instance/boot-finished";
        let command = format!("sudo test -f {cloud_init_finished_marker}");

        while Instant::now() <= deadline {
            let finished = match self.syncer.run_remote(context.networking, &command) {
                Ok(output) => matches!(output.exit_code, Some(0)),
                Err(err) => {
                    let message = self
                        .teardown_with_note(context.instance, Some(context.networking), &err)
                        .await;
                    return Err(RunError::Provisioning {
                        message,
                        source: err,
                    });
                }
            };

            if finished {
                return Ok(());
            }

            sleep(self.cloud_init_poll_interval).await;
        }

        let timeout_message = format!(
            "cloud-init did not finish within {} seconds",
            self.cloud_init_wait_timeout.as_secs()
        );
        let message_with_teardown = self
            .teardown_with_note(context.instance, Some(context.networking), &timeout_message)
            .await;
        Err(RunError::ProvisioningTimeout {
            message: message_with_teardown,
        })
    }

    /// Unmounts the cache path when a volume was configured.
    ///
    /// Returns a description of the failure, if any. The caller still requests
    /// the power-off fallback, which teardown performs; this note keeps the
    /// unmount outcome visible when teardown also fails.
    pub(super) fn unmount_cache(
        &self,
        networking: &InstanceNetworking,
        volume_id: Option<&str>,
    ) -> Option<String> {
        let id = volume_id?;
        let mount_path = &self.syncer.config().volume_mount_path;
        let script = cache_device::unmount_script(id, mount_path);

        match self.syncer.run_remote_raw(networking, &script) {
            Ok(output) => match cache_device::parse_report(&output.stdout) {
                Ok(report) if report.status() == &CacheDeviceStatus::Unmounted => None,
                Ok(report) => Some(format!(
                    "unmount of {mount_path} for volume {id} reported {}",
                    report.describe()
                )),
                Err(error) => Some(format!(
                    "unmount of {mount_path} for volume {id} produced no usable status: {error}"
                )),
            },
            Err(error) => Some(format!(
                "unmount request for {mount_path} (volume {id}) failed: {error}"
            )),
        }
    }

    /// Tears the instance down and returns `err` as text, with a note appended
    /// when the teardown also fails.
    ///
    /// A configured cache is flushed and unmounted first; teardown then stops
    /// the server, detaches the volume, waits for it to become `available`, and
    /// only then deletes the server.
    ///
    /// For example, `teardown` failing yields `"<err> (teardown also failed:
    /// ...)"` rather than discarding the original error.
    pub(super) async fn teardown_with_note<E: Display>(
        &self,
        instance: ProvisionedInstance<'_>,
        networking: Option<&InstanceNetworking>,
        err: &E,
    ) -> String {
        let mut message = err.to_string();
        if let Some(reachable) = networking
            && let Some(note) = self.unmount_cache(reachable, instance.volume_id)
        {
            message = format!("{message} ({note})");
        }

        let teardown_error = self
            .backend
            .teardown(instance.handle, instance.volume_id)
            .await
            .err();
        append_teardown_note(message, teardown_error.as_ref())
    }
}
