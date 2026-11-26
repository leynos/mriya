//! Scaleway backend implementation of the instance lifecycle.
//!
//! Creation records every confirmed allocation in the run's ownership ledger.
//! Cleanup works from that ledger: it shuts the server down, detaches the
//! volumes mriya must preserve, deletes the server, then deletes the owned
//! volumes and public IPs through the API that owns each one, verifying every
//! postcondition by an identity-checked listing.

mod cleanup;
mod cleanup_support;
mod cleanup_volumes;
mod error;
mod inventory;
mod lifecycle;
mod types;
mod volume;

use std::time::Duration;

use scaleway_rs::ScalewayApi;
use types::Zone;

use crate::{
    backend::{Backend, BackendFuture, InstanceHandle, InstanceNetworking, InstanceRequest},
    cleanup::{CleanupFailure, CleanupReport},
    config::ScalewayConfig,
    janitor::{TEST_RUN_ID_ENV, TEST_RUN_TAG_PREFIX},
    ownership::{Disposition, ResourceLedger, ResourceRef},
    scaleway::lifecycle::{records::record_server_creation, volumes::CacheAttachment},
    volume::{VolumeBackend, VolumeHandle, VolumeRequest},
};

/// Port assumed for SSH readiness probes and the returned networking details.
const DEFAULT_SSH_PORT: u16 = 22;
/// Pause between provider polls and SSH connection attempts.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Overall budget for each readiness or teardown wait (five minutes).
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

pub use error::ScalewayBackendError;
pub use inventory::ResourceInventory;

/// Backend that provisions instances through the Scaleway Instances API.
#[derive(Clone)]
pub struct ScalewayBackend {
    /// SDK client used for image lookups; every other call uses direct HTTP.
    api: ScalewayApi,
    /// Base URL for the Instances API calls made by direct HTTP request.
    /// Overridable within the crate so tests can target a loopback server.
    instance_api_base: String,
    /// Base URL for the Block Storage API calls made by direct HTTP request.
    /// Overridable within the crate so tests can target a loopback server.
    block_api_base: String,
    /// Validated configuration; supplies the secret key and default request
    /// values.
    config: ScalewayConfig,
    /// Identifier added as a tag to created resources so a test run can find
    /// and clean up its own leftovers.
    test_run_id: Option<String>,
    /// Port reported in networking details and probed for SSH readiness.
    ssh_port: u16,
    /// Delay between polls while waiting for readiness or deletion.
    poll_interval: Duration,
    /// Deadline applied to each wait before a timeout error is returned.
    wait_timeout: Duration,
    /// Test-only seam that replaces the SDK image lookup, so a lifecycle test
    /// can drive the whole create path against a loopback API.
    #[cfg(test)]
    image_id_override: Option<String>,
}

impl ScalewayBackend {
    /// Classifies API errors that identify the requested instance type.
    /// For example, a matching `resource_id` is sufficient even when the
    /// error's resource is `server`.
    fn is_instance_type_error(
        api_err: &scaleway_rs::ScalewayApiError,
        request: &InstanceRequest,
    ) -> bool {
        matches!(api_err.resource.as_deref(), Some("commercial_type"))
            || api_err
                .resource_id
                .as_deref()
                .is_some_and(|id| id == request.instance_type)
    }

    /// Constructs a new backend from configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when the provided configuration
    /// fails validation.
    pub fn new(config: ScalewayConfig) -> Result<Self, ScalewayBackendError> {
        Self::new_with_env(config, &mockable::DefaultEnv)
    }

    /// Constructs a new backend, reading the test run ID from `env`.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when the provided configuration
    /// fails validation.
    pub fn new_with_env(
        config: ScalewayConfig,
        env: &impl mockable::Env,
    ) -> Result<Self, ScalewayBackendError> {
        Self::new_with_test_run_id(config, env.string(TEST_RUN_ID_ENV))
    }

    /// Constructs a new backend with an explicit test run ID.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when the provided configuration
    /// fails validation.
    pub fn new_with_test_run_id(
        config: ScalewayConfig,
        test_run_id: Option<String>,
    ) -> Result<Self, ScalewayBackendError> {
        config.validate()?;
        Ok(Self {
            api: ScalewayApi::new(&config.secret_key),
            instance_api_base: lifecycle::SCALEWAY_INSTANCE_API_BASE.to_owned(),
            block_api_base: lifecycle::SCALEWAY_BLOCK_API_BASE.to_owned(),
            config,
            test_run_id,
            ssh_port: DEFAULT_SSH_PORT,
            poll_interval: POLL_INTERVAL,
            wait_timeout: WAIT_TIMEOUT,
            #[cfg(test)]
            image_id_override: None,
        })
    }

    /// Resolves the boot image, honouring the test-only resolver seam.
    ///
    /// The seam exists because the SDK's image lookup hard-codes the
    /// production host. With it unset, this is exactly
    /// [`ScalewayBackend::resolve_image_id`].
    async fn resolve_image_for_create(
        &self,
        request: &InstanceRequest,
    ) -> Result<String, ScalewayBackendError> {
        #[cfg(test)]
        if let Some(image_id) = self.image_id_override.as_deref() {
            return Ok(image_id.to_owned());
        }
        self.resolve_image_id(request).await
    }

    /// Builds an instance request using the backend's defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when configuration validation
    /// fails.
    pub fn default_request(&self) -> Result<InstanceRequest, ScalewayBackendError> {
        self.default_request_with_env(&mockable::DefaultEnv)
    }

    /// Builds an instance request using the backend's defaults, reading `HOME`
    /// for a `~/` cloud-init file path through `env`.
    ///
    /// This is [`ScalewayBackend::default_request`] with an injectable
    /// environment, so a caller or test can resolve a tilde path without
    /// reading the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when configuration validation
    /// fails.
    pub fn default_request_with_env(
        &self,
        env: &impl mockable::Env,
    ) -> Result<InstanceRequest, ScalewayBackendError> {
        self.config
            .as_request_with_env(env)
            .map_err(ScalewayBackendError::from)
    }

    /// Rejects a cache volume that is the instance's root volume.
    ///
    /// IDs are compared after trimming whitespace. Attaching the root volume
    /// again as the cache volume would corrupt the volume map, so the request
    /// fails with `VolumeAttachmentFailed` instead.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Padding around the identifier is ignored when comparing.
    /// let err =
    ///     ScalewayBackend::validate_cache_volume_id(" vol-1 ", "vol-1", &handle).unwrap_err();
    /// assert!(matches!(err, ScalewayBackendError::VolumeAttachmentFailed { .. }));
    ///
    /// // Two different volumes pass, so the PATCH may proceed.
    /// ScalewayBackend::validate_cache_volume_id("vol-cache", "vol-1", &handle)?;
    /// ```
    fn validate_cache_volume_id(
        cache_volume_id: &str,
        root_volume_id: &str,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        if cache_volume_id.trim() == root_volume_id.trim() {
            return Err(ScalewayBackendError::VolumeAttachmentFailed {
                volume_id: cache_volume_id.trim().to_owned(),
                instance_id: handle.id.clone(),
                message: String::from("refuse to attach root volume as cache volume"),
            });
        }

        Ok(())
    }

    /// Appends the test-run tag and allocation tag, in that order.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// assert_eq!(
    ///     ScalewayBackend::build_tags(vec![String::from("mriya")], Some(" abc "), None),
    ///     vec![String::from("mriya"), format!("{TEST_RUN_TAG_PREFIX}abc")]
    /// );
    /// ```
    fn build_tags(
        base_tags: Vec<String>,
        test_run_id: Option<&str>,
        allocation: Option<&str>,
    ) -> Vec<String> {
        let mut tags = base_tags;
        if let Some(tag) = allocation.map(str::trim).filter(|tag| !tag.is_empty()) {
            tags.push(tag.to_owned());
        }
        if let Some(id) = test_run_id.map(str::trim).filter(|id| !id.is_empty()) {
            tags.push(format!("{TEST_RUN_TAG_PREFIX}{id}"));
        }
        tags
    }

    /// Tags applied to created instances: `mriya`, `ephemeral`, the
    /// allocation tag, and the optional test-run tag.
    fn instance_tags(test_run_id: Option<&str>, allocation: Option<&str>) -> Vec<String> {
        Self::build_tags(
            vec![String::from("mriya"), String::from("ephemeral")],
            test_run_id,
            allocation,
        )
    }

    /// Tags applied to created cache volumes: `mriya`, `cache`, the
    /// allocation tag, and the optional test-run tag.
    fn volume_tags(test_run_id: Option<&str>, allocation: Option<&str>) -> Vec<String> {
        Self::build_tags(
            vec![String::from("mriya"), String::from("cache")],
            test_run_id,
            allocation,
        )
    }
}

impl Backend for ScalewayBackend {
    type Error = ScalewayBackendError;

    fn create<'a>(
        &'a self,
        request: &'a InstanceRequest,
        ledger: &'a ResourceLedger,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        Box::pin(async move {
            request.validate()?;
            ledger
                .persist_initial()
                .map_err(|error| ScalewayBackendError::Journal {
                    message: error.message,
                })?;

            let image_id = self.resolve_image_for_create(request).await?;
            ledger.record(
                ResourceRef::image(&image_id, &request.zone),
                Disposition::Borrowed,
            );

            let server = match self
                .create_instance_stopped(request, &image_id, ledger)
                .await
            {
                Ok(server) => server,
                Err(error @ ScalewayBackendError::AllocationOutcomeUnknown { .. }) => {
                    ledger.mark_outcome_unknown();
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            record_server_creation(&server, ledger);

            let handle = InstanceHandle {
                id: server.id.clone(),
                zone: request.zone.clone(),
            };

            if let Some(volume_id) = request.volume_id.as_deref() {
                let root_volume_id = server
                    .root_volume()
                    .map(|volume| volume.id.clone())
                    .ok_or_else(|| ScalewayBackendError::VolumeNotFound {
                        volume_id: String::from("0"),
                        zone: request.zone.clone(),
                    })?;
                Self::validate_cache_volume_id(volume_id, &root_volume_id, &handle)?;
                let family = self
                    .record_configured_cache(volume_id, &request.zone, ledger)
                    .await?;
                self.attach_volume(
                    &handle,
                    CacheAttachment {
                        volume_id,
                        root_volume_id: &root_volume_id,
                        family,
                    },
                )
                .await?;
            }

            let snapshot = server.snapshot();
            self.power_on_if_needed(&Zone::from(request.zone.as_str()), &snapshot)
                .await?;

            // Powering on is when a routed public IP is attached. Recording it
            // now keeps it owned rather than letting cleanup classify it as a
            // borrowed address it must preserve. A refresh failure is not
            // fatal: the server and root volume are already recorded, and the
            // run can continue.
            if let Ok(refreshed) = self.get_server(&request.zone, &handle.id).await {
                record_server_creation(&refreshed, ledger);
            }

            Ok(handle)
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        Box::pin(async move {
            let networking = self.wait_for_public_ip(handle).await?;
            self.wait_for_ssh_ready(handle, &networking).await?;
            Ok(networking)
        })
    }

    fn cleanup<'a>(
        &'a self,
        ledger: &'a ResourceLedger,
    ) -> BackendFuture<'a, CleanupReport, CleanupFailure> {
        Box::pin(async move { self.cleanup_ledger(ledger).await })
    }
}

impl VolumeBackend for ScalewayBackend {
    fn create_volume<'a>(
        &'a self,
        request: &'a VolumeRequest,
        ledger: &'a ResourceLedger,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error> {
        Box::pin(async move { Self::create_volume(self, request, ledger).await })
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
