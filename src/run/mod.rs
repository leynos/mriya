//! Orchestrates end-to-end remote runs over SSH.
//!
//! The run workflow provisions an instance via a backend, waits for SSH
//! readiness, synchronizes the local workspace, executes a remote command
//! using the system `ssh` client, and tears the instance down. Remote exit
//! codes are preserved so callers observe the same status locally.

use std::{fmt::Display, time::Duration};

use camino::Utf8Path;

use crate::{
    backend::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest},
    sync::{CacheRouting, CommandRunner, RemoteCommandOutput, Syncer},
    volume::VolumeBackend,
};

/// Delay between checks for the cloud-init completion marker.
const CLOUD_INIT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Total time to wait for cloud-init to finish (ten minutes).
const CLOUD_INIT_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

mod error;
mod steps;

pub use error::RunError;

/// Executes the remote run flow using the provided backend and syncer.
#[derive(Debug)]
pub struct RunOrchestrator<B, R: CommandRunner> {
    /// Backend that creates, waits for, and destroys the instance.
    backend: B,
    /// Syncer used for workspace transfer and remote command execution.
    syncer: Syncer<R>,
    /// Delay between cloud-init completion checks.
    cloud_init_poll_interval: Duration,
    /// Time allowed for cloud-init before a `ProvisioningTimeout` error.
    cloud_init_wait_timeout: Duration,
}

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend + VolumeBackend,
    B::Error: Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
{
    /// Creates a new orchestrator.
    ///
    /// # Parameters
    ///
    /// - `backend`: provisions, waits for and tears down the instance.
    /// - `syncer`: transfers the workspace and runs remote commands.
    ///
    /// # Returns
    ///
    /// A new orchestrator with the default cloud-init timing.
    #[must_use]
    pub const fn new(backend: B, syncer: Syncer<R>) -> Self {
        Self {
            backend,
            syncer,
            cloud_init_poll_interval: CLOUD_INIT_POLL_INTERVAL,
            cloud_init_wait_timeout: CLOUD_INIT_WAIT_TIMEOUT,
        }
    }

    /// Overrides the cloud-init polling interval.
    ///
    /// This is primarily used by tests to keep timeout scenarios fast.
    ///
    /// # Parameters
    ///
    /// - `interval`: delay between cloud-init completion checks.
    ///
    /// # Returns
    ///
    /// The orchestrator with the interval replaced.
    #[must_use]
    pub const fn with_cloud_init_poll_interval(mut self, interval: Duration) -> Self {
        self.cloud_init_poll_interval = interval;
        self
    }

    /// Overrides the cloud-init wait timeout.
    ///
    /// This is primarily used by tests to keep timeout scenarios fast.
    ///
    /// # Parameters
    ///
    /// - `timeout`: total time allowed for cloud-init to finish.
    ///
    /// # Returns
    ///
    /// The orchestrator with the timeout replaced.
    #[must_use]
    pub const fn with_cloud_init_wait_timeout(mut self, timeout: Duration) -> Self {
        self.cloud_init_wait_timeout = timeout;
        self
    }

    /// Runs the end-to-end workflow and returns the remote command output.
    ///
    /// The remote exit code is returned even when non-zero. Teardown is
    /// always attempted; when teardown fails the error is surfaced even if
    /// the remote command succeeded.
    ///
    /// # Parameters
    ///
    /// - `request`: instance parameters, including an optional cache volume.
    /// - `source`: local working tree to synchronize.
    /// - `remote_command`: command to execute over SSH.
    ///
    /// # Returns
    ///
    /// The remote command's output. A non-zero remote exit code is preserved
    /// when teardown succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] when provisioning, readiness checks, cache mount
    /// verification, synchronization, remote execution, or teardown fail.
    pub async fn execute(
        &self,
        request: &InstanceRequest,
        source: &Utf8Path,
        remote_command: &str,
    ) -> Result<RemoteCommandOutput, RunError<B::Error>> {
        let handle = self
            .backend
            .create(request)
            .await
            .map_err(|error| RunError::Provision(Box::new(error)))?;
        let instance = ProvisionedInstance {
            handle: &handle,
            volume_id: request.volume_id.as_deref(),
        };
        let networking = self.wait_for_ready_or_destroy(instance).await?;

        let context = StepContext {
            instance,
            networking: &networking,
        };
        let cache = self.mount_volume_if_needed(context, request).await?;

        let dest = self.syncer.destination_for(&networking);
        self.sync_or_destroy(context, source, &dest).await?;

        if request.cloud_init_user_data.is_some() {
            self.wait_for_cloud_init(context).await?;
        }

        let routing = cache
            .as_ref()
            .map_or(CacheRouting::Unconfigured, |verified| {
                CacheRouting::Required {
                    device: verified.device.clone(),
                }
            });
        let output = self
            .run_remote_or_destroy(context, remote_command, &routing)
            .await?;

        let unmount_note = self.unmount_cache(&networking, instance.volume_id);
        if let Err(failure) = self
            .backend
            .teardown(instance.handle, instance.volume_id)
            .await
        {
            let stage = failure.stage.as_str();
            let teardown_source = failure.source;
            let message = unmount_note.map_or_else(
                || format!("{teardown_source} while {stage}"),
                |note| format!("{teardown_source} while {stage} ({note})"),
            );
            return Err(RunError::Teardown {
                message,
                source: Box::new(teardown_source),
            });
        }

        Ok(output)
    }

    /// Waits for the backend to report SSH readiness, destroying the instance
    /// if that fails.
    ///
    /// The teardown outcome is appended to the error message.
    ///
    /// For example, an SSH timeout returns its own error with
    /// `(teardown also failed: ...)` appended when the destroy also fails, so
    /// the caller sees both problems rather than only the first.
    async fn wait_for_ready_or_destroy(
        &self,
        instance: ProvisionedInstance<'_>,
    ) -> Result<InstanceNetworking, RunError<B::Error>> {
        match self.backend.wait_for_ready(instance.handle).await {
            Ok(net) => Ok(net),
            Err(err) => {
                let message = self.teardown_with_note(instance, None, &err).await;
                Err(RunError::Wait {
                    message,
                    source: Box::new(err),
                })
            }
        }
    }
}

/// Instance references shared by the steps after provisioning.
#[derive(Clone, Copy)]
struct ProvisionedInstance<'a> {
    /// Server created for this run.
    handle: &'a InstanceHandle,
    /// Configured cache volume, when one was attached.
    volume_id: Option<&'a str>,
}

/// Cache mount verified before the user command runs.
#[derive(Clone, Debug)]
pub(super) struct VerifiedCache {
    /// Guest device path the mount script resolved and verified.
    pub(super) device: String,
}

/// A provisioned instance plus its reachable networking details.
#[derive(Clone, Copy)]
struct StepContext<'a> {
    /// Instance created for this run.
    instance: ProvisionedInstance<'a>,
    /// Public address and port used for SSH.
    networking: &'a InstanceNetworking,
}
