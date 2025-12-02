//! Scaleway backend implementation of the instance lifecycle.

use std::net::IpAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use crate::backend::{
    Backend, BackendError, BackendFuture, InstanceHandle, InstanceNetworking, InstanceRequest,
};
use crate::config::{ConfigError, ScalewayConfig};
use scaleway_rs::{
    ScalewayApi, ScalewayCreateInstanceBuilder, ScalewayError, ScalewayListInstanceImagesBuilder,
};
use thiserror::Error;
use tokio::time::sleep;
use uuid::Uuid;

const DEFAULT_SSH_PORT: u16 = 22;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Eq, PartialEq)]
struct InstanceSnapshot {
    id: String,
    state: String,
    allowed_actions: Vec<String>,
    public_ip: Option<String>,
}

/// Errors raised by the Scaleway backend.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScalewayBackendError {
    /// Raised when the high-level configuration is incomplete.
    #[error("configuration error: {0}")]
    Config(String),
    /// Raised when a request is missing a required field.
    #[error("invalid instance request: {0}")]
    Validation(String),
    /// Raised when the requested image label cannot be resolved.
    #[error("image '{label}' (arch {arch}) not found in zone {zone}")]
    ImageNotFound {
        /// Image label passed by the caller.
        label: String,
        /// Architecture requested by the caller.
        arch: String,
        /// Zone used for the lookup.
        zone: String,
    },
    /// Raised when the server type is not available in the selected zone.
    #[error("instance type '{instance_type}' not available in zone {zone}")]
    InstanceTypeUnavailable {
        /// Requested commercial type.
        instance_type: String,
        /// Target zone.
        zone: String,
    },
    /// Raised when an asynchronous operation exceeds the timeout.
    #[error("timeout waiting for {action} on instance {instance_id}")]
    Timeout {
        /// Action being waited on.
        action: String,
        /// Provider instance identifier.
        instance_id: String,
    },
    /// Raised when the instance never exposes a public IP.
    #[error("instance {instance_id} missing public IPv4 address")]
    MissingPublicIp {
        /// Provider instance identifier.
        instance_id: String,
    },
    /// Raised when teardown leaves a server visible in the API.
    #[error("instance {instance_id} still present after teardown")]
    ResidualResource {
        /// Provider instance identifier.
        instance_id: String,
    },
    /// Raised when an instance cannot be powered on.
    #[error("instance {instance_id} in state {state} cannot be powered on")]
    PowerOnNotAllowed {
        /// Provider instance identifier.
        instance_id: String,
        /// Current state reported by the provider.
        state: String,
    },
    /// Wrapper for provider level failures.
    #[error("provider error: {message}")]
    Provider {
        /// Message returned by the provider SDK.
        message: String,
    },
}

impl From<ScalewayError> for ScalewayBackendError {
    fn from(value: ScalewayError) -> Self {
        Self::Provider {
            message: value.to_string(),
        }
    }
}

impl From<BackendError> for ScalewayBackendError {
    fn from(value: BackendError) -> Self {
        match value {
            BackendError::Validation(field) => Self::Validation(field),
        }
    }
}

impl From<ConfigError> for ScalewayBackendError {
    fn from(value: ConfigError) -> Self {
        Self::Config(value.to_string())
    }
}

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

    async fn resolve_image_id(
        &self,
        request: &InstanceRequest,
    ) -> Result<String, ScalewayBackendError> {
        let mut images = if request.project_id.is_empty() {
            ScalewayListInstanceImagesBuilder::new(self.api.clone(), &request.zone)
                .public(true)
                .name(&request.image_label)
                .arch(&request.architecture)
                .run_async()
                .await?
        } else {
            let mut scoped =
                ScalewayListInstanceImagesBuilder::new(self.api.clone(), &request.zone)
                    .public(true)
                    .project(&request.project_id)
                    .name(&request.image_label)
                    .arch(&request.architecture);
            if let Some(org) = &request.organisation_id {
                scoped = scoped.organization(org);
            }
            let project_images = scoped.run_async().await?;
            if project_images.is_empty() {
                ScalewayListInstanceImagesBuilder::new(self.api.clone(), &request.zone)
                    .public(true)
                    .name(&request.image_label)
                    .arch(&request.architecture)
                    .run_async()
                    .await?
            } else {
                project_images
            }
        };

        let mut candidates: Vec<_> = images
            .drain(..)
            .filter(|image| image.arch == request.architecture)
            .filter(|image| image.state == "available")
            .collect();

        if candidates.is_empty() {
            return Err(ScalewayBackendError::ImageNotFound {
                label: request.image_label.clone(),
                arch: request.architecture.clone(),
                zone: request.zone.clone(),
            });
        }

        candidates.sort_by(|lhs, rhs| rhs.creation_date.cmp(&lhs.creation_date));
        let image_id = candidates.remove(0).id;
        Ok(image_id)
    }

    async fn power_on_if_needed(
        &self,
        zone: &str,
        snapshot: &InstanceSnapshot,
    ) -> Result<(), ScalewayBackendError> {
        if snapshot.state == "running" {
            return Ok(());
        }

        if snapshot
            .allowed_actions
            .iter()
            .any(|action| action == "poweron")
        {
            self.api
                .perform_instance_action_async(zone, &snapshot.id, "poweron")
                .await?;
            return Ok(());
        }

        Err(ScalewayBackendError::PowerOnNotAllowed {
            instance_id: snapshot.id.clone(),
            state: snapshot.state.clone(),
        })
    }

    async fn fetch_instance(
        &self,
        handle: &InstanceHandle,
    ) -> Result<Option<InstanceSnapshot>, ScalewayBackendError> {
        let mut servers = self
            .api
            .list_instances(&handle.zone)
            .servers(&handle.id)
            .per_page(1)
            .run_async()
            .await?;

        Ok(servers.pop().map(|server| InstanceSnapshot {
            id: server.id,
            state: server.state,
            allowed_actions: server.allowed_actions,
            public_ip: server.public_ip.map(|ip| ip.address),
        }))
    }

    async fn wait_for_public_ip(
        &self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            if Instant::now() > deadline {
                return Err(ScalewayBackendError::Timeout {
                    action: "wait_for_ready".to_owned(),
                    instance_id: handle.id.clone(),
                });
            }

            let Some(server) = self.fetch_instance(handle).await? else {
                sleep(self.poll_interval).await;
                continue;
            };

            if server.state != "running" {
                sleep(self.poll_interval).await;
                continue;
            }

            if let Some(address) = server
                .public_ip
                .as_ref()
                .and_then(|ip| IpAddr::from_str(ip).ok())
            {
                return Ok(InstanceNetworking {
                    public_ip: address,
                    ssh_port: self.ssh_port,
                });
            }

            if Instant::now() > deadline {
                return Err(ScalewayBackendError::MissingPublicIp {
                    instance_id: handle.id.clone(),
                });
            }

            sleep(self.poll_interval).await;
        }
    }

    async fn wait_until_gone(&self, handle: &InstanceHandle) -> Result<(), ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        while Instant::now() <= deadline {
            if self.fetch_instance(handle).await?.is_none() {
                return Ok(());
            }
            sleep(self.poll_interval).await;
        }

        Err(ScalewayBackendError::ResidualResource {
            instance_id: handle.id.clone(),
        })
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
                    if api_err.resource.as_deref() == Some("commercial_type") =>
                {
                    return Err(ScalewayBackendError::InstanceTypeUnavailable {
                        instance_type: request.instance_type.clone(),
                        zone: request.zone.clone(),
                    });
                }
                Err(other) => return Err(other.into()),
            };

            let snapshot = InstanceSnapshot {
                id: server.id.clone(),
                state: server.state.clone(),
                allowed_actions: server.allowed_actions.clone(),
                public_ip: server.public_ip.as_ref().map(|ip| ip.address.clone()),
            };

            self.power_on_if_needed(&request.zone, &snapshot).await?;

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
