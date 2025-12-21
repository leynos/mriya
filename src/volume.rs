//! Volume lifecycle abstractions for persistent cache storage.

use crate::backend::{Backend, BackendFuture, InstanceHandle};

/// Parameters required to create a persistent volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeRequest {
    /// Human-friendly volume name.
    pub name: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Target availability zone.
    pub zone: String,
    /// Project identifier used for billing and ownership.
    pub project_id: String,
    /// Optional organisation identifier when the provider requires one.
    pub organisation_id: Option<String>,
}

impl VolumeRequest {
    /// Creates a new volume request, trimming string fields.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        size_bytes: u64,
        zone: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into().trim().to_owned(),
            size_bytes,
            zone: zone.into().trim().to_owned(),
            project_id: project_id.into().trim().to_owned(),
            organisation_id: None,
        }
    }

    /// Sets the optional organisation identifier.
    #[must_use]
    pub fn organisation_id(mut self, value: Option<String>) -> Self {
        self.organisation_id = value.map(|id| id.trim().to_owned());
        self
    }
}

/// Handle returned after creating a volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeHandle {
    /// Provider-specific volume identifier.
    pub id: String,
    /// Zone where the volume was created.
    pub zone: String,
}

/// Backend operations required for volume management.
pub trait VolumeBackend: Backend {
    /// Creates a new volume and returns its handle.
    fn create_volume<'a>(
        &'a self,
        request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error>;

    /// Detaches a volume from the given instance.
    fn detach_volume<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error>;
}
