//! Scaleway backend implementation of the instance lifecycle.
//!
//! Server operations use the Instances API, including the dedicated
//! `attach-volume` and `detach-volume` endpoints. Cache volumes are created,
//! read, polled and deleted through the Block Storage v1 API, because Scaleway
//! retired new Block Storage creation through the Instances API in April 2025.

mod error;
mod lifecycle;
mod types;
mod volume;

use std::time::Duration;

use scaleway_rs::ScalewayApi;

use crate::{
    backend::{Backend, BackendFuture, InstanceHandle, InstanceNetworking, InstanceRequest},
    config::ScalewayConfig,
    janitor::{TEST_RUN_ID_ENV, TEST_RUN_TAG_PREFIX},
    volume::{VolumeBackend, VolumeHandle, VolumeRequest, VolumeState, VolumeStatus},
};

/// Port assumed for SSH readiness probes and the returned networking details.
const DEFAULT_SSH_PORT: u16 = 22;
/// Pause between provider polls and SSH connection attempts.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Overall budget for each readiness or teardown wait (five minutes).
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

pub use error::ScalewayBackendError;

/// Backend that provisions instances through the Scaleway Instances API and
/// manages cache volumes through the Block Storage API.
#[derive(Clone)]
pub struct ScalewayBackend {
    /// SDK client used for image lookups; every other call is direct HTTP.
    api: ScalewayApi,
    /// Base URL for the Instances API calls made by direct HTTP request.
    /// Overridable within the crate so tests can target a loopback server.
    instance_api_base: String,
    /// Base URL for the Block Storage v1 API. Overridable within the crate so
    /// loopback tests can serve both APIs from one listener.
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
}

impl ScalewayBackend {
    /// Classifies API errors that identify the requested instance type.
    /// For example, a matching `resource_id` is sufficient even when the
    /// error's resource is `server`.
    fn is_instance_type_error(
        api_err: &scaleway_rs::ScalewayApiError,
        request: &InstanceRequest,
    ) -> bool {
        // A former third clause (`etype == "invalid_arguments"` with a
        // `commercial_type` resource) was subsumed by the first check and
        // has been removed: whenever it held, the first clause had already
        // classified the error.
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
    /// This is [`ScalewayBackend::new`] with an injectable environment: the
    /// [`TEST_RUN_ID_ENV`] variable is looked up through `env` rather than the
    /// process environment.
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
        })
    }

    /// Builds an instance request using the backend's defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when configuration validation
    /// fails.
    pub fn default_request(&self) -> Result<InstanceRequest, ScalewayBackendError> {
        self.config.as_request().map_err(ScalewayBackendError::from)
    }

    /// Appends the test-run tag to `base_tags` when `test_run_id` is non-blank.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // A blank identifier adds nothing; a real one is appended after a trim.
    /// assert_eq!(
    ///     ScalewayBackend::build_tags(vec![String::from("mriya")], None),
    ///     vec![String::from("mriya")]
    /// );
    /// assert_eq!(
    ///     ScalewayBackend::build_tags(vec![String::from("mriya")], Some(" abc ")),
    ///     vec![String::from("mriya"), format!("{TEST_RUN_TAG_PREFIX}abc")]
    /// );
    /// ```
    fn build_tags(base_tags: Vec<String>, test_run_id: Option<&str>) -> Vec<String> {
        let mut tags = base_tags;
        let Some(id) = test_run_id else {
            return tags;
        };
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return tags;
        }
        tags.push(format!("{TEST_RUN_TAG_PREFIX}{trimmed}"));
        tags
    }

    /// Tags applied to created instances: `mriya`, `ephemeral`, and the
    /// optional test-run tag.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// assert_eq!(
    ///     ScalewayBackend::instance_tags(None),
    ///     vec![String::from("mriya"), String::from("ephemeral")]
    /// );
    /// ```
    fn instance_tags(test_run_id: Option<&str>) -> Vec<String> {
        Self::build_tags(
            vec![String::from("mriya"), String::from("ephemeral")],
            test_run_id,
        )
    }

    /// Tags applied to created cache volumes: `mriya`, `cache`, and the
    /// optional test-run tag.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// assert_eq!(
    ///     ScalewayBackend::volume_tags(None),
    ///     vec![String::from("mriya"), String::from("cache")]
    /// );
    /// ```
    fn volume_tags(test_run_id: Option<&str>) -> Vec<String> {
        Self::build_tags(
            vec![String::from("mriya"), String::from("cache")],
            test_run_id,
        )
    }
}

impl Backend for ScalewayBackend {
    type Error = ScalewayBackendError;

    fn create<'a>(
        &'a self,
        request: &'a InstanceRequest,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        Box::pin(async move {
            request.validate()?;
            let image_id = self.resolve_image_id(request).await?;
            self.create_with_image_id(request, &image_id).await
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

    fn destroy(&self, handle: InstanceHandle) -> BackendFuture<'_, (), Self::Error> {
        Box::pin(async move {
            self.delete_instance(&handle).await?;
            self.wait_until_gone(&handle).await
        })
    }
}

impl VolumeBackend for ScalewayBackend {
    fn create_volume<'a>(
        &'a self,
        request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error> {
        Box::pin(async move { Self::create_volume(self, request).await })
    }

    fn get_volume<'a>(
        &'a self,
        zone: &'a str,
        volume_id: &'a str,
    ) -> BackendFuture<'a, VolumeState, Self::Error> {
        Box::pin(async move { Self::get_volume(self, zone, volume_id).await })
    }

    fn wait_for_volume_status<'a>(
        &'a self,
        zone: &'a str,
        volume_id: &'a str,
        status: VolumeStatus,
    ) -> BackendFuture<'a, VolumeState, Self::Error> {
        Box::pin(async move { Self::wait_for_volume_status(self, zone, volume_id, status).await })
    }

    fn delete_volume<'a>(&'a self, handle: &'a VolumeHandle) -> BackendFuture<'a, (), Self::Error> {
        Box::pin(async move { Self::delete_volume(self, handle).await })
    }

    fn attach_volume<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        Box::pin(async move {
            let instance = self.get_instance(handle).await?;
            let expected = volume::expected_map(&instance.volumes.volumes);
            Self::attach_volume(self, handle, volume_id, &expected).await
        })
    }

    fn detach_volume<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        Box::pin(async move { Self::detach_volume(self, handle, volume_id).await })
    }

    fn teardown<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: Option<&'a str>,
    ) -> BackendFuture<'a, (), Self::Error> {
        Box::pin(async move { Self::teardown_instance(self, handle, volume_id).await })
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
