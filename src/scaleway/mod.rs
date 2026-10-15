//! Scaleway backend implementation of the instance lifecycle.

mod error;
mod lifecycle;
mod types;
mod volume;

use std::time::Duration;

use crate::backend::{Backend, BackendFuture, InstanceHandle, InstanceNetworking, InstanceRequest};
use crate::config::ScalewayConfig;
use crate::volume::{VolumeBackend, VolumeHandle, VolumeRequest};
use lifecycle::InstanceSnapshot;
use scaleway_rs::ScalewayApi;
use types::{Action, Zone};

use crate::janitor::{TEST_RUN_ID_ENV, TEST_RUN_TAG_PREFIX};

const DEFAULT_SSH_PORT: u16 = 22;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

pub use error::ScalewayBackendError;

/// Backend that provisions instances through the Scaleway Instances API.
#[derive(Clone)]
pub struct ScalewayBackend {
    api: ScalewayApi,
    instance_api_base: String,
    config: ScalewayConfig,
    test_run_id: Option<String>,
    ssh_port: u16,
    poll_interval: Duration,
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
        let test_run_id = std::env::var(TEST_RUN_ID_ENV).ok();
        Self::new_with_test_run_id(config, test_run_id)
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

    fn instance_tags(test_run_id: Option<&str>) -> Vec<String> {
        Self::build_tags(
            vec![String::from("mriya"), String::from("ephemeral")],
            test_run_id,
        )
    }

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

            let server = self.create_instance_stopped(request, &image_id).await?;

            let handle = InstanceHandle {
                id: server.id.clone(),
                zone: request.zone.clone(),
            };

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
                Self::validate_cache_volume_id(volume_id, &root_volume_id, &handle)?;
                self.attach_volume(&handle, volume_id, root_volume_id)
                    .await?;
            }

            let snapshot = InstanceSnapshot {
                id: server.id.into(),
                state: server.state.into(),
                allowed_actions: server
                    .allowed_actions
                    .into_iter()
                    .map(Action::from)
                    .collect(),
                public_ip: server.public_ip.as_ref().map(|ip| ip.address.clone()),
            };

            let zone = Zone::from(request.zone.as_str());
            self.power_on_if_needed(&zone, &snapshot).await?;

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

    fn detach_volume<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        Box::pin(async move { Self::detach_volume(self, handle, volume_id).await })
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for Scaleway backend tagging, error classification, and
    //! volume validation.
    //!
    //! Kills the error-classification and cache-volume validation survivors
    //! tracked in #57.
    use rstest::rstest;

    use super::ScalewayBackend;
    use crate::backend::{InstanceHandle, InstanceRequest};
    use crate::scaleway::ScalewayBackendError;

    /// Builds a provider error with the supplied classification fields.
    fn api_error(
        etype: &str,
        resource: Option<&str>,
        resource_id: Option<&str>,
    ) -> scaleway_rs::ScalewayApiError {
        scaleway_rs::ScalewayApiError {
            message: String::from("boom"),
            resource: resource.map(str::to_owned),
            resource_id: resource_id.map(str::to_owned),
            etype: etype.to_owned(),
        }
    }

    /// Creates a request whose instance type can be varied by a test case.
    fn request_with_type(instance_type: &str) -> InstanceRequest {
        InstanceRequest {
            image_label: String::from("label"),
            instance_type: instance_type.to_owned(),
            zone: String::from("zone"),
            project_id: String::from("proj"),
            organisation_id: None,
            architecture: String::from("x86_64"),
            volume_id: None,
            cloud_init_user_data: None,
        }
    }

    /// Supplies the stable instance identity used by volume error assertions.
    fn handle() -> InstanceHandle {
        InstanceHandle {
            id: String::from("instance-1"),
            zone: String::from("zone"),
        }
    }

    #[derive(Clone, Copy)]
    struct CacheVolumeCase {
        cache_volume_id: &'static str,
        root_volume_id: &'static str,
        expect_rejection: bool,
    }

    type TagBuilder = fn(Option<&str>) -> Vec<String>;

    #[derive(Clone, Copy)]
    struct TagCase {
        builder: TagBuilder,
        base_tag: &'static str,
        test_run_id: Option<&'static str>,
        expected_test_tag: Option<&'static str>,
    }

    /// Covers both type-identifying fields and unrelated provider errors.
    #[rstest]
    // Resource alone identifies the instance type, without a resource_id.
    #[case(api_error("invalid_arguments", Some("commercial_type"), None), true)]
    // A resource_id matching the requested type classifies even when the
    // resource is unrelated.
    #[case(api_error("unknown", Some("server"), Some("DEV1-S")), true)]
    // A non-matching resource_id must not classify.
    #[case(api_error("unknown", Some("server"), Some("GP1-XS")), false)]
    // An unrelated error must not classify.
    #[case(api_error("out_of_stock", None, None), false)]
    fn is_instance_type_error_classifies(
        #[case] api_err: scaleway_rs::ScalewayApiError,
        #[case] expected: bool,
    ) {
        let request = request_with_type("DEV1-S");
        assert_eq!(
            ScalewayBackend::is_instance_type_error(&api_err, &request),
            expected,
            "unexpected classification for {api_err:?}"
        );
    }

    /// Rejects equal trimmed volume IDs while accepting distinct IDs.
    #[rstest]
    #[case::same_trimmed_id(CacheVolumeCase {
        cache_volume_id: " vol-1 ", root_volume_id: "vol-1", expect_rejection: true,
    })]
    #[case::distinct_id(CacheVolumeCase {
        cache_volume_id: "vol-1", root_volume_id: "vol-2", expect_rejection: false,
    })]
    fn validate_cache_volume_id_rejects_only_the_root_volume(#[case] case: CacheVolumeCase) {
        let result = ScalewayBackend::validate_cache_volume_id(
            case.cache_volume_id,
            case.root_volume_id,
            &handle(),
        );
        let rejected = matches!(
            result,
            Err(ScalewayBackendError::VolumeAttachmentFailed { .. })
        );
        assert_eq!(
            rejected, case.expect_rejection,
            "cache={:?} root={:?}: {result:?}",
            case.cache_volume_id, case.root_volume_id
        );
    }

    /// Includes test-run tags only for nonblank, trimmed identifiers.
    #[rstest]
    #[case::instance_absent(TagCase {
        builder: ScalewayBackend::instance_tags, base_tag: "ephemeral",
        test_run_id: None, expected_test_tag: None,
    })]
    #[case::volume_absent(TagCase {
        builder: ScalewayBackend::volume_tags, base_tag: "cache",
        test_run_id: None, expected_test_tag: None,
    })]
    #[case::instance_blank(TagCase {
        builder: ScalewayBackend::instance_tags, base_tag: "ephemeral",
        test_run_id: Some("   "), expected_test_tag: None,
    })]
    #[case::volume_blank(TagCase {
        builder: ScalewayBackend::volume_tags, base_tag: "cache",
        test_run_id: Some("\t\n"), expected_test_tag: None,
    })]
    #[case::instance_nonblank(TagCase {
        builder: ScalewayBackend::instance_tags, base_tag: "ephemeral",
        test_run_id: Some("run-123"), expected_test_tag: Some("mriya-test-run-run-123"),
    })]
    #[case::volume_nonblank(TagCase {
        builder: ScalewayBackend::volume_tags, base_tag: "cache",
        test_run_id: Some("run-123"), expected_test_tag: Some("mriya-test-run-run-123"),
    })]
    #[case::instance_padded(TagCase {
        builder: ScalewayBackend::instance_tags, base_tag: "ephemeral",
        test_run_id: Some("  run-123  "), expected_test_tag: Some("mriya-test-run-run-123"),
    })]
    #[case::volume_padded(TagCase {
        builder: ScalewayBackend::volume_tags, base_tag: "cache",
        test_run_id: Some("\t run-123\n"), expected_test_tag: Some("mriya-test-run-run-123"),
    })]
    fn tags_include_test_run_tag_only_for_nonblank_id(#[case] case: TagCase) {
        let mut expected_tags = vec![String::from("mriya"), String::from(case.base_tag)];
        if let Some(test_tag) = case.expected_test_tag {
            expected_tags.push(String::from(test_tag));
        }
        assert_eq!(
            (case.builder)(case.test_run_id),
            expected_tags,
            "base={:?} id={:?}",
            case.base_tag,
            case.test_run_id
        );
    }
}
