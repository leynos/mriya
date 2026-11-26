//! Backend abstraction for provisioning disposable compute instances.

use std::{future::Future, net::IpAddr, pin::Pin};

use thiserror::Error;

use crate::cloud_init;

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
    pub volume_id: Option<String>,
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
    /// # Errors
    ///
    /// Returns [`BackendError::Validation`] when a required string field is
    /// empty or `cloud_init_user_data` is not valid, and
    /// [`BackendError::VolumeIdNotUuid`] when `volume_id` is present but is not
    /// a canonical hyphenated UUID. Match the UUID variant when the distinction
    /// matters to the caller; both are rejected before any provider request.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::InstanceRequest;
    ///
    /// let request = InstanceRequest::builder()
    ///     .image_label("ubuntu")
    ///     .instance_type("DEV1-S")
    ///     .zone("fr-par-1")
    ///     .project_id("project")
    ///     .architecture("x86_64")
    ///     .volume_id(Some(String::from("not-a-uuid")))
    ///     .build();
    /// assert!(request.is_err());
    /// ```
    pub fn validate(&self) -> Result<(), BackendError> {
        if let Some(field) = self.first_missing_field() {
            return Err(BackendError::Validation(field.to_owned()));
        }
        if let Some(volume_id) = self.volume_id.as_deref()
            && !is_canonical_uuid(volume_id)
        {
            return Err(BackendError::VolumeIdNotUuid {
                value: volume_id.to_owned(),
            });
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

    /// Returns the first required string field that is empty, if any.
    ///
    /// # Returns
    ///
    /// The field name used in [`BackendError::Validation`], or `None` when all
    /// required fields are present.
    fn first_missing_field(&self) -> Option<&'static str> {
        [
            ("image_label", self.image_label.as_str()),
            ("instance_type", self.instance_type.as_str()),
            ("zone", self.zone.as_str()),
            ("project_id", self.project_id.as_str()),
            ("architecture", self.architecture.as_str()),
        ]
        .into_iter()
        .find(|(_, value)| value.is_empty())
        .map(|(field, _)| field)
    }
}

/// Reports whether `value` is a canonical hyphenated UUID.
///
/// Block Storage identifiers are UUIDs, and the value is later interpolated
/// into a guest shell command, so the shape is checked before it travels.
///
/// # Examples
///
/// ```
/// use mriya::backend::is_canonical_uuid;
///
/// assert!(is_canonical_uuid("11111111-2222-3333-4444-555555555555"));
/// assert!(!is_canonical_uuid("11111111222233334444555555555555"));
/// assert!(!is_canonical_uuid(
///     "11111111-2222-3333-4444-55555555555; rm -rf /"
/// ));
/// ```
#[must_use]
pub fn is_canonical_uuid(value: &str) -> bool {
    !value.is_empty()
        && value.len() == 36
        && value
            .chars()
            .enumerate()
            .all(|(index, character)| match index {
                8 | 13 | 18 | 23 => character == '-',
                _ => character.is_ascii_hexdigit(),
            })
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
    /// Raised when a volume identifier is not a canonical UUID.
    #[error(
        "volume_id '{value}' is not a canonical UUID; use the hyphenated identifier returned by `mriya init` or listed by `scw block volume list`"
    )]
    VolumeIdNotUuid {
        /// Identifier that failed validation.
        value: String,
    },
}

/// Future returned by backend operations.
pub type BackendFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// Minimal interface implemented by cloud backends.
pub trait Backend {
    /// Provider specific error type returned by the backend.
    type Error: std::error::Error + Send + Sync + 'static;

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
