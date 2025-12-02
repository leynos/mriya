//! Scaleway backend implementation of the instance lifecycle.

mod error;
mod lifecycle;
mod types;

use std::time::Duration;

use crate::backend::{Backend, BackendFuture, InstanceHandle, InstanceNetworking, InstanceRequest};
use crate::config::ScalewayConfig;
use lifecycle::InstanceSnapshot;
use scaleway_rs::{ScalewayApi, ScalewayCreateInstanceBuilder, ScalewayError};
use types::{Action, Zone};
use uuid::Uuid;

const DEFAULT_SSH_PORT: u16 = 22;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

pub use error::ScalewayBackendError;

/// Backend that provisions instances through the Scaleway Instances API.
#[derive(Clone)]
pub struct ScalewayBackend {
    api: ScalewayApi,
    config: ScalewayConfig,
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
    }

    /// Constructs a new backend from configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when the provided configuration
    /// fails validation.
    pub fn new(config: ScalewayConfig) -> Result<Self, ScalewayBackendError> {
        config.validate()?;
        Ok(Self {
            api: ScalewayApi::new(&config.secret_key),
            config,
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
            let server = match ScalewayCreateInstanceBuilder::new(
                self.api.clone(),
                &request.zone,
                &name,
                &request.instance_type,
            )
            .image(&image_id)
            .project(&request.project_id)
            .routed_ip_enabled(true)
            .tags(vec![String::from("mriya"), String::from("ephemeral")])
            .run_async()
            .await
            {
                Ok(server) => server,
                Err(ScalewayError::Api(api_err))
                    if Self::is_instance_type_error(&api_err, request)
                        || (api_err.etype == "invalid_arguments"
                            && api_err
                                .message
                                .to_ascii_lowercase()
                                .contains("commercial_type")) =>
                {
                    return Err(ScalewayBackendError::InstanceTypeUnavailable {
                        instance_type: request.instance_type.clone(),
                        zone: request.zone.clone(),
                    });
                }
                Err(other) => return Err(other.into()),
            };

            let snapshot = InstanceSnapshot {
                id: server.id.clone().into(),
                state: server.state.clone().into(),
                allowed_actions: server
                    .allowed_actions
                    .clone()
                    .into_iter()
                    .map(Action::from)
                    .collect(),
                public_ip: server.public_ip.as_ref().map(|ip| ip.address.clone()),
            };

            let zone = Zone::from(request.zone.as_str());
            self.power_on_if_needed(&zone, &snapshot).await?;

            Ok(InstanceHandle {
                id: server.id,
                zone: request.zone.clone(),
            })
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        Box::pin(async move { self.wait_for_public_ip(handle).await })
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
