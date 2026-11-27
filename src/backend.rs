//! Backend abstraction for provisioning disposable compute instances.

use std::{future::Future, net::IpAddr, pin::Pin};

use thiserror::Error;

use crate::{
    boot_volume::{BootVolumeAllocation, BootVolumePlan, BootVolumeRequest},
    cloud_init,
};

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
    /// Optional organization identifier when the provider requires one.
    pub organisation_id: Option<String>,
    /// CPU architecture requested for the instance.
    pub architecture: String,
    /// Optional volume ID to attach for persistent storage.
    ///
    /// This is the retained cache volume. It is never the boot volume: the
    /// boot disk is described by [`InstanceRequest::boot_volume`].
    pub volume_id: Option<String>,
    /// Boot-storage settings for the instance's root volume.
    ///
    /// Unset settings follow the backend's documented compatibility default,
    /// which the resolved [`BootVolumePlan`] records.
    pub boot_volume: BootVolumeRequest,
    /// Optional cloud-init user-data payload (cloud-config YAML or script).
    ///
    /// When provided, the backend passes the blob through to the provider so
    /// cloud-init can provision packages and other system configuration before
    /// the remote command begins.
    pub cloud_init_user_data: Option<String>,
}

impl InstanceRequest {
    /// Starts a builder for an [`InstanceRequest`].
    #[must_use]
    pub fn builder() -> InstanceRequestBuilder { InstanceRequestBuilder::new() }

    /// Validates the request, returning a descriptive error when a required
    /// field is missing.
    ///
    /// The required fields are checked in a fixed order, so a request with
    /// several problems reports the first one by name rather than an
    /// arbitrary one. For example, an empty image label and an empty zone
    /// report `image_label`.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Validation`] when any required string field is
    /// empty, when the boot size is zero, or when the cloud-init payload is
    /// not valid user-data.
    pub fn validate(&self) -> Result<(), BackendError> {
        let required = [
            ("image_label", self.image_label.as_str()),
            ("instance_type", self.instance_type.as_str()),
            ("zone", self.zone.as_str()),
            ("project_id", self.project_id.as_str()),
            ("architecture", self.architecture.as_str()),
        ];
        for (field, value) in required {
            if value.is_empty() {
                return Err(BackendError::Validation(field.to_owned()));
            }
        }
        self.validate_optional_fields()
    }

    /// Validates the settings that are allowed to be absent.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Validation`] when a supplied boot size is zero,
    /// or when a supplied cloud-init payload is not valid user-data.
    fn validate_optional_fields(&self) -> Result<(), BackendError> {
        if self.boot_volume.size_gb == Some(0) {
            return Err(BackendError::Validation("boot_volume_size_gb".to_owned()));
        }
        if let Some(payload) = self.cloud_init_user_data.as_deref()
            && cloud_init::validate_payload(payload).is_err()
        {
            return Err(BackendError::Validation(String::from(
                "cloud_init_user_data",
            )));
        }
        Ok(())
    }
}

/// Builder for [`InstanceRequest`] that defers trimming and validation to
/// construction.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstanceRequestBuilder {
    /// Image label; trimmed on build.
    image_label: String,
    /// Instance type; trimmed on build.
    instance_type: String,
    /// Availability zone; trimmed on build.
    zone: String,
    /// Project identifier; trimmed on build.
    project_id: String,
    /// Optional organization identifier; trimmed on build when present.
    organisation_id: Option<String>,
    /// CPU architecture; trimmed on build.
    architecture: String,
    /// Optional cache volume ID; trimmed on build when present.
    volume_id: Option<String>,
    /// Boot-storage settings; carried through unchanged.
    boot_volume: BootVolumeRequest,
    /// Optional cloud-init payload; kept verbatim, with no trimming.
    cloud_init_user_data: Option<String>,
}

impl InstanceRequestBuilder {
    /// Creates an empty builder; fields must be populated before build.
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Sets the image label.
    #[must_use]
    pub fn image_label(mut self, value: impl Into<String>) -> Self {
        self.image_label = value.into();
        self
    }

    /// Sets the instance type.
    #[must_use]
    pub fn instance_type(mut self, value: impl Into<String>) -> Self {
        self.instance_type = value.into();
        self
    }

    /// Sets the availability zone.
    #[must_use]
    pub fn zone(mut self, value: impl Into<String>) -> Self {
        self.zone = value.into();
        self
    }

    /// Sets the project identifier.
    #[must_use]
    pub fn project_id(mut self, value: impl Into<String>) -> Self {
        self.project_id = value.into();
        self
    }

    /// Sets the optional organization identifier.
    #[must_use]
    pub fn organisation_id(mut self, value: Option<String>) -> Self {
        self.organisation_id = value;
        self
    }

    /// Sets the architecture.
    #[must_use]
    pub fn architecture(mut self, value: impl Into<String>) -> Self {
        self.architecture = value.into();
        self
    }

    /// Sets the optional volume ID for persistent storage.
    #[must_use]
    pub fn volume_id(mut self, value: Option<String>) -> Self {
        self.volume_id = value;
        self
    }

    /// Sets the boot-storage settings for the instance's root volume.
    ///
    /// For example, a builder given a 20 GB local request sends that plan,
    /// while one given [`BootVolumeRequest::default`] accepts the backend's
    /// compatibility default.
    #[must_use]
    pub const fn boot_volume(mut self, value: BootVolumeRequest) -> Self {
        self.boot_volume = value;
        self
    }

    /// Sets optional cloud-init user-data content.
    #[must_use]
    pub fn cloud_init_user_data(mut self, value: Option<String>) -> Self {
        self.cloud_init_user_data = value;
        self
    }

    /// Builds and validates the [`InstanceRequest`], trimming string inputs.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Validation`] when any required field is empty.
    pub fn build(self) -> Result<InstanceRequest, BackendError> {
        let request = InstanceRequest {
            image_label: self.image_label.trim().to_owned(),
            instance_type: self.instance_type.trim().to_owned(),
            zone: self.zone.trim().to_owned(),
            project_id: self.project_id.trim().to_owned(),
            organisation_id: self.organisation_id.map(|value| value.trim().to_owned()),
            architecture: self.architecture.trim().to_owned(),
            volume_id: self.volume_id.map(|value| value.trim().to_owned()),
            boot_volume: self.boot_volume,
            cloud_init_user_data: self.cloud_init_user_data,
        };
        request.validate()?;
        Ok(request)
    }
}

/// Handle returned by a backend once an instance has been created.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceHandle {
    /// Provider specific identifier for the instance.
    pub id: String,
    /// Zone in which the instance was created.
    pub zone: String,
    /// The boot volume the instance actually reported.
    ///
    /// Absent only for a backend or a test double that records no root
    /// volume; a real Scaleway create always fills it in.
    pub boot_volume: Option<BootVolumeAllocation>,
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

/// Future returned by backend operations.
pub type BackendFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// Minimal interface implemented by cloud backends.
pub trait Backend {
    /// Provider specific error type returned by the backend.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Resolves and validates the boot-volume plan without creating anything.
    ///
    /// Callers use this to reject an impossible combination before a billable
    /// resource exists. A preflight is advisory: `create` applies the same
    /// rules again on its own inputs, and a standalone `create` never depends
    /// on a caller having preflighted first. If the catalogue or image list
    /// changes between the two calls, the create's resolution is the one that
    /// governs the server.
    ///
    /// For example, a 20 GB local plan succeeds where the instance type
    /// offers local storage and the image fits, while a size above the type's
    /// maximum fails before any request is made.
    fn preflight<'a>(
        &'a self,
        request: &'a InstanceRequest,
    ) -> BackendFuture<'a, BootVolumePlan, Self::Error>;

    /// Creates a new instance and returns a handle used for subsequent calls.
    ///
    /// For example, a valid request yields a handle whose identifier and zone
    /// are then passed to every later call, while an unavailable instance type
    /// returns the backend's error instead of a handle.
    fn create<'a>(
        &'a self,
        request: &'a InstanceRequest,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error>;

    /// Blocks until the instance is ready for SSH and returns networking info.
    ///
    /// For example, an instance that becomes reachable yields its public IP
    /// and SSH port, while one that never does returns the backend's timeout
    /// error.
    fn wait_for_ready<'a>(
        &'a self,
        handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error>;

    /// Destroys the instance and ensures no provider resources remain.
    ///
    /// For example, a successful teardown resolves once the instance is gone
    /// from the provider's listing, while a provider rejection returns its
    /// error so the caller can report a possible leak.
    fn destroy(&self, handle: InstanceHandle) -> BackendFuture<'_, (), Self::Error>;
}
