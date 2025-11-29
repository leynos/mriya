//! Backend abstraction for provisioning disposable compute instances.

use std::net::IpAddr;

use thiserror::Error;

/// Parameters required to create a new instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceRequest {
    /// Human readable label used for the boot image. The backend resolves this
    /// to a provider specific image identifier.
    pub image_label: String,
    /// Commercial type or flavour to request (for example `DEV1-S`).
    pub instance_type: String,
    /// Target availability zone (for example `fr-par-1`).
    pub zone: String,
    /// Project identifier used for billing and ownership.
    pub project_id: String,
    /// Optional organisation identifier when the provider requires one.
    pub organisation_id: Option<String>,
    /// CPU architecture requested for the instance.
    pub architecture: String,
}

impl InstanceRequest {
    /// Creates a new request, trimming inputs to avoid accidental whitespace.
    #[must_use]
    pub fn new(
        image_label: impl Into<String>,
        instance_type: impl Into<String>,
        zone: impl Into<String>,
        project_id: impl Into<String>,
        organisation_id: Option<String>,
        architecture: impl Into<String>,
    ) -> Self {
        Self {
            image_label: image_label.into().trim().to_owned(),
            instance_type: instance_type.into().trim().to_owned(),
            zone: zone.into().trim().to_owned(),
            project_id: project_id.into().trim().to_owned(),
            organisation_id: organisation_id.map(|value| value.trim().to_owned()),
            architecture: architecture.into().trim().to_owned(),
        }
    }

    /// Validates the request, returning a descriptive error when a required
    /// field is missing.
    #[must_use]
    pub fn validate(&self) -> Result<(), BackendError> {
        if self.image_label.is_empty() {
            return Err(BackendError::Validation("image_label".to_owned()));
        }
        if self.instance_type.is_empty() {
            return Err(BackendError::Validation("instance_type".to_owned()));
        }
        if self.zone.is_empty() {
            return Err(BackendError::Validation("zone".to_owned()));
        }
        if self.project_id.is_empty() {
            return Err(BackendError::Validation("project_id".to_owned()));
        }
        if self.architecture.is_empty() {
            return Err(BackendError::Validation("architecture".to_owned()));
        }
        Ok(())
    }
}

/// Handle returned by a backend once an instance has been created.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceHandle {
    /// Provider specific identifier for the instance.
    pub id: String,
    /// Zone in which the instance was created.
    pub zone: String,
}

/// Connection details for reaching an instance once it is ready.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceNetworking {
    /// Public IPv4 address assigned by the provider.
    pub public_ip: IpAddr,
    /// TCP port for SSH (defaults to 22 on Scaleway).
    pub ssh_port: u16,
}

/// Errors raised by backends.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum BackendError {
    /// Raised when a request is missing a required field.
    #[error("missing or empty field: {0}")]
    Validation(String),
}

/// Minimal interface implemented by cloud backends.
#[allow(async_fn_in_trait)]
pub trait Backend {
    /// Provider specific error type returned by the backend.
    type Error: std::error::Error;

    /// Creates a new instance and returns a handle used for subsequent calls.
    async fn create(&self, request: &InstanceRequest) -> Result<InstanceHandle, Self::Error>;

    /// Blocks until the instance is ready for SSH and returns networking info.
    async fn wait_for_ready(
        &self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, Self::Error>;

    /// Destroys the instance and ensures no provider resources remain.
    async fn destroy(&self, handle: InstanceHandle) -> Result<(), Self::Error>;
}
