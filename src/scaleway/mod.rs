//! Scaleway backend implementation of the instance lifecycle.

mod backend_impl;
mod error;
mod lifecycle;
mod types;
mod volume;

use std::time::Duration;

use backend_impl::{PROVIDER_NAME, may_have_created};
use lifecycle::InstanceSnapshot;
use scaleway_rs::ScalewayApi;
use types::{Action, Zone};

use crate::{
    backend::{InstanceHandle, InstanceRequest},
    config::ScalewayConfig,
    janitor::{TEST_RUN_ID_ENV, TEST_RUN_TAG_PREFIX},
};

/// Port assumed for SSH readiness probes and the returned networking details.
const DEFAULT_SSH_PORT: u16 = 22;
/// Pause between provider polls and SSH connection attempts.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Overall budget for each readiness or teardown wait (five minutes).
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

pub use error::ScalewayBackendError;

/// Backend that provisions instances through the Scaleway Instances API.
#[derive(Clone)]
pub struct ScalewayBackend {
    /// SDK client used for image lookups; instance, volume, and user-data calls
    /// bypass it and use direct HTTP requests.
    api: ScalewayApi,
    /// Base URL for the Instances API calls made by direct HTTP request.
    /// Overridable within the crate so tests can target a loopback server.
    instance_api_base: String,
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
    /// Optional benchmark sink that receives the identifiers of the resources
    /// this backend creates and tears down.
    ///
    /// `None` on every ordinary run. The sink is constructor-injected rather
    /// than added to the `Backend` port because only a benchmark needs it, and
    /// a port that returned provider identifiers would force every future
    /// adapter to invent them.
    lifecycle_sink: Option<crate::bench::lifecycle::SharedLifecycleSink>,
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
            config,
            test_run_id,
            ssh_port: DEFAULT_SSH_PORT,
            poll_interval: POLL_INTERVAL,
            wait_timeout: WAIT_TIMEOUT,
            lifecycle_sink: None,
        })
    }

    /// Attaches a benchmark lifecycle sink.
    ///
    /// The sink receives the identifier of every resource this backend creates
    /// and the outcome of every teardown. Nothing reads back from it, so an
    /// attached sink cannot change what the backend does.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let sink = Arc::new(RecordingSink::default());
    /// let backend = ScalewayBackend::new(config)?.with_lifecycle_sink(sink);
    /// ```
    #[must_use]
    pub fn with_lifecycle_sink(
        mut self,
        sink: crate::bench::lifecycle::SharedLifecycleSink,
    ) -> Self {
        self.lifecycle_sink = Some(sink);
        self
    }

    /// Reports an event to the configured sink, if any.
    fn notify_lifecycle(&self, event: &crate::bench::lifecycle::LifecycleEvent) {
        crate::bench::lifecycle::notify(self.lifecycle_sink.as_ref(), event);
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

    /// Creates a stopped instance and reports the outcome to the sink.
    ///
    /// The provider's create call is the point at which a resource may begin
    /// to bill. A response that cannot be read or parsed therefore reports an
    /// uncertain allocation even though the call returned an error, because
    /// the request may well have been applied.
    async fn create_instance_stopped_observed(
        &self,
        request: &InstanceRequest,
        image_id: &str,
    ) -> Result<scaleway_rs::ScalewayInstance, ScalewayBackendError> {
        use crate::bench::lifecycle::LifecycleEvent;

        match self.create_instance_stopped(request, image_id).await {
            Ok(server) => {
                self.notify_lifecycle(&LifecycleEvent::ResourceCreated {
                    provider: String::from(PROVIDER_NAME),
                    resource_id: server.id.clone(),
                    location: request.zone.clone(),
                    product: request.instance_type.clone(),
                });
                Ok(server)
            }
            Err(err) => {
                if may_have_created(&err) {
                    self.notify_lifecycle(&LifecycleEvent::AllocationUncertain {
                        provider: String::from(PROVIDER_NAME),
                        location: request.zone.clone(),
                        reason: err.to_string(),
                    });
                }
                Err(err)
            }
        }
    }

    /// Attaches the requested cache volume and powers the instance on.
    ///
    /// Split from [`Backend::create`] so the failure path can report the
    /// resource it leaves behind without repeating the finish steps.
    async fn finish_creation(
        &self,
        request: &InstanceRequest,
        server: &scaleway_rs::ScalewayInstance,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        // Attach cache volume before powering on (instance is stopped)
        if let Some(ref volume_id) = request.volume_id {
            let root_volume_id = server
                .volumes
                .volumes
                .get("0")
                .map(|v| v.id.clone())
                .ok_or_else(|| ScalewayBackendError::VolumeNotFound {
                    volume_id: String::from("0"),
                    zone: request.zone.clone(),
                })?;
            Self::validate_cache_volume_id(volume_id, &root_volume_id, handle)?;
            self.attach_volume(handle, volume_id, root_volume_id)
                .await?;
        }

        let snapshot = InstanceSnapshot {
            id: server.id.clone().into(),
            state: server.state.clone().into(),
            allowed_actions: server
                .allowed_actions
                .iter()
                .map(|action| Action::from(action.clone()))
                .collect(),
            public_ip: server.public_ip.as_ref().map(|ip| ip.address.clone()),
        };

        let zone = Zone::from(request.zone.as_str());
        self.power_on_if_needed(&zone, &snapshot).await
    }

    /// Reports the resources a failed creation leaves behind and returns the
    /// original error unchanged.
    fn abandon_creation(
        &self,
        handle: &InstanceHandle,
        err: ScalewayBackendError,
    ) -> ScalewayBackendError {
        use crate::bench::lifecycle::LifecycleEvent;
        self.notify_lifecycle(&LifecycleEvent::AllocationUncertain {
            provider: String::from(PROVIDER_NAME),
            location: handle.zone.clone(),
            reason: format!(
                "instance {} was created but could not be finished: {err}",
                handle.id
            ),
        });
        err
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
