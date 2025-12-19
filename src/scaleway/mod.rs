//! Scaleway backend implementation of the instance lifecycle.

mod error;
mod lifecycle;
mod types;
mod user_data;
mod volume;

use std::time::Duration;

use crate::backend::{Backend, BackendFuture, InstanceHandle, InstanceNetworking, InstanceRequest};
use crate::config::ScalewayConfig;
use lifecycle::InstanceSnapshot;
use scaleway_rs::{ScalewayApi, ScalewayCreateInstanceBuilder, ScalewayError};
use types::{Action, Zone};
use uuid::Uuid;

use crate::janitor::{TEST_RUN_ID_ENV, TEST_RUN_TAG_PREFIX};

const DEFAULT_SSH_PORT: u16 = 22;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

pub use error::ScalewayBackendError;

/// Backend that provisions instances through the Scaleway Instances API.
#[derive(Clone)]
pub struct ScalewayBackend {
    api: ScalewayApi,
    config: ScalewayConfig,
    test_run_id: Option<String>,
    ssh_port: u16,
    poll_interval: Duration,
    wait_timeout: Duration,
}

impl ScalewayBackend {
    fn is_instance_type_error(
        api_err: &scaleway_rs::ScalewayApiError,
        request: &InstanceRequest,
    ) -> bool {
        matches!(api_err.resource.as_deref(), Some("commercial_type"))
            || api_err
                .resource_id
                .as_deref()
                .is_some_and(|id| id == request.instance_type)
            || (api_err.etype == "invalid_arguments"
                && matches!(api_err.resource.as_deref(), Some("commercial_type")))
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

    fn instance_tags(test_run_id: Option<&str>) -> Vec<String> {
        let mut tags = vec![String::from("mriya"), String::from("ephemeral")];
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

            let name = format!("mriya-{}", Uuid::new_v4().simple());
            let tags = Self::instance_tags(self.test_run_id.as_deref());
            let server = match ScalewayCreateInstanceBuilder::new(
                self.api.clone(),
                &request.zone,
                &name,
                &request.instance_type,
            )
            .image(&image_id)
            .project(&request.project_id)
            .routed_ip_enabled(true)
            .tags(tags)
            .run_async()
            .await
            {
                Ok(server) => server,
                Err(ScalewayError::Api(api_err))
                    if Self::is_instance_type_error(&api_err, request) =>
                {
                    return Err(ScalewayBackendError::InstanceTypeUnavailable {
                        instance_type: request.instance_type.clone(),
                        zone: request.zone.clone(),
                    });
                }
                Err(other) => return Err(other.into()),
            };

            let handle = InstanceHandle {
                id: server.id.clone(),
                zone: request.zone.clone(),
            };

            // Apply cloud-init user-data before the first boot.
            if let Some(ref user_data) = request.cloud_init_user_data {
                self.set_cloud_init_user_data(&handle, user_data).await?;
            }

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
            self.api
                .delete_instance_async(&handle.zone, &handle.id)
                .await?;
            self.wait_until_gone(&handle).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ScalewayBackend;

    #[test]
    fn instance_tags_omits_test_tag_when_unset() {
        let tags = ScalewayBackend::instance_tags(None);
        assert_eq!(tags, vec![String::from("mriya"), String::from("ephemeral")]);
    }

    #[test]
    fn instance_tags_adds_test_run_tag() {
        let tags = ScalewayBackend::instance_tags(Some("run-123"));
        assert_eq!(
            tags,
            vec![
                String::from("mriya"),
                String::from("ephemeral"),
                String::from("mriya-test-run-run-123"),
            ]
        );
    }
}
