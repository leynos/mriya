//! The individual phases of a run, each destroying the instance on failure.
//!
//! Every helper here either completes its phase or tears the instance down and
//! returns the error with the teardown outcome appended. Keeping them together
//! makes that invariant reviewable in one place: no helper may return an error
//! while leaving an instance running.

use std::{fmt::Display, time::Instant};

use camino::Utf8Path;
use shell_escape::unix::escape;
use tokio::time::sleep;

use super::{RunError, RunOrchestrator};
use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest},
    bench::{observe::Observation, record::PhaseKind},
    sync::{CommandRunner, RemoteCommandOutput, create_cache_directories_command},
    teardown::append_teardown_note,
};

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend,
    B::Error: std::fmt::Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
{
    /// Waits for the backend to report SSH readiness, destroying the instance
    /// if that fails.
    pub(super) async fn wait_for_ready_or_destroy(
        &self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, RunError<B::Error>> {
        match self
            .observe_async(PhaseKind::Bootstrap, self.backend.wait_for_ready(handle))
            .await
        {
            Ok(net) => Ok(net),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(RunError::Wait {
                    message,
                    source: err,
                })
            }
        }
    }

    /// Mounts the cache volume only when the request attached one.
    ///
    /// For example, a request carrying a `volume_id` mounts it, while one
    /// without an attached volume returns `Ok(())` without contacting the
    /// instance at all.
    pub(super) async fn mount_volume_if_needed(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        request: &InstanceRequest,
    ) -> Result<(), RunError<B::Error>> {
        if request.volume_id.is_some() {
            self.observe_async(
                PhaseKind::CacheRestore,
                self.mount_cache_volume(handle, networking),
            )
            .await?;
        }
        Ok(())
    }

    /// Syncs the workspace to `dest`, destroying the instance if that fails.
    ///
    /// For example, a failed rsync returns the sync error with the teardown
    /// outcome appended, so a leaked instance is never reported as a plain
    /// sync failure.
    pub(super) async fn sync_or_destroy(
        &self,
        handle: &InstanceHandle,
        source: &Utf8Path,
        dest: &crate::sync::SyncDestination,
    ) -> Result<(), RunError<B::Error>> {
        let synced = self.observe_sync(PhaseKind::Sync, || self.syncer.sync(source, dest));
        if let Err(err) = synced {
            let message = self.destroy_with_note(handle, &err).await;
            return Err(RunError::Sync {
                message,
                source: err,
            });
        }
        Ok(())
    }

    /// Runs `remote_command`, destroying the instance only if it could not be
    /// started.
    ///
    /// A non-zero exit code from the command is a successful result here.
    ///
    /// For example, a command exiting 7 returns that status in
    /// [`RemoteCommandOutput`], while a failure to start the command destroys
    /// the instance and returns the error text.
    pub(super) async fn run_remote_or_destroy(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, RunError<B::Error>> {
        match self.observe_sync(PhaseKind::RemoteExecution, || {
            self.syncer.run_remote(networking, remote_command)
        }) {
            Ok(result) => Ok(result),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(RunError::Remote {
                    message,
                    source: err,
                })
            }
        }
    }

    /// Mounts the cache volume via SSH and creates cache subdirectories.
    ///
    /// The mount command is idempotent: it creates the mount point directory
    /// and attempts to mount `/dev/vdb`. The mount itself is best-effort
    /// because the command uses `|| true` for graceful degradation. Only SSH
    /// execution failures are surfaced as errors.
    ///
    /// When `create_cache_directories` is enabled in the sync configuration,
    /// the cache subdirectories are created after mounting so that language
    /// toolchains can write immediately.
    ///
    /// For example, with cache directories enabled and the path already a
    /// mount point, `/mnt/mriya-cache/npm` and its siblings are created before
    /// the remote command runs, so a toolchain usually does not have to create
    /// them itself.
    ///
    /// Creation is only *attempted*, never guaranteed. The guard
    /// `mountpoint -q` tests whether the path *is* a mount point rather than
    /// whether this mount attempt succeeded, so an earlier mount can satisfy
    /// it; when the path is not a mount point at all, nothing is attempted.
    /// The creation command itself ends in `2>/dev/null || true`, so a
    /// read-only or full filesystem also leaves no directories without failing
    /// the run. A toolchain must therefore cope with their absence.
    pub(super) async fn mount_cache_volume(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
    ) -> Result<(), RunError<B::Error>> {
        let config = self.syncer.config();
        let mount_path = &config.volume_mount_path;
        let escaped_mount_path = escape(mount_path.as_str().into());

        let mkdir_cache_dirs = if config.create_cache_directories {
            format!(
                " && if mountpoint -q {path} 2>/dev/null; then {create_dirs}; fi",
                path = escaped_mount_path,
                create_dirs = create_cache_directories_command(mount_path)
            )
        } else {
            String::new()
        };

        let mount_command = format!(
            concat!(
                "sudo mkdir -p {path} && ",
                "sudo mount /dev/vdb {path} 2>/dev/null || true"
            ),
            path = escaped_mount_path
        );

        let full_command = format!("{mount_command}{mkdir_cache_dirs}");

        match self.syncer.run_remote_raw(networking, &full_command) {
            Ok(_) => Ok(()),
            Err(err) => {
                let message = self.destroy_with_note(handle, &err).await;
                Err(RunError::Sync {
                    message,
                    source: err,
                })
            }
        }
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
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
    ) -> Result<(), RunError<B::Error>> {
        // Cloud-init readiness is part of bootstrap: it is the guest still
        // becoming usable, and the protocol times it there rather than opening
        // an eighth phase for a wait that only some runs perform.
        self.observer
            .observe(Observation::PhaseStarted(PhaseKind::Bootstrap));
        let started = Instant::now();
        let outcome = self.poll_cloud_init(handle, networking).await;
        let succeeded = outcome.is_ok();
        self.observer.observe(Observation::PhaseFinished {
            phase: PhaseKind::Bootstrap,
            duration: started.elapsed(),
            succeeded,
        });
        outcome
    }

    /// Polls for cloud-init's completion marker, destroying the instance on
    /// failure or timeout.
    ///
    /// Split from [`RunOrchestrator::wait_for_cloud_init`] so the observation
    /// wraps every exit path, including the ones that destroy the instance.
    pub(super) async fn poll_cloud_init(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
    ) -> Result<(), RunError<B::Error>> {
        let deadline = Instant::now() + self.cloud_init_wait_timeout;
        let cloud_init_finished_marker = "/var/lib/cloud/instance/boot-finished";
        let command = format!("sudo test -f {cloud_init_finished_marker}");

        while Instant::now() <= deadline {
            let finished = match self.syncer.run_remote(networking, &command) {
                Ok(output) => matches!(output.exit_code, Some(0)),
                Err(err) => {
                    let message = self.destroy_with_note(handle, &err).await;
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
        let message_with_teardown = self.destroy_with_note(handle, &timeout_message).await;
        Err(RunError::ProvisioningTimeout {
            message: message_with_teardown,
        })
    }

    /// Destroys the instance and returns `err` as text, with a note appended
    /// when the teardown also fails.
    ///
    /// For example, `destroy` failing yields `"<err> (teardown also failed:
    /// ...)"` rather than discarding the original error.
    pub(super) async fn destroy_with_note<E: Display>(
        &self,
        handle: &InstanceHandle,
        err: &E,
    ) -> String {
        let teardown_error = self.backend.destroy(handle.clone()).await.err();
        append_teardown_note(err.to_string(), teardown_error.as_ref())
    }
}
