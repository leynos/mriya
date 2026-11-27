//! Orchestrated instance creation with cache preflight and rollback.
//!
//! A configured cache volume is validated before any server exists: a volume in
//! another zone, another project, the legacy storage class, an attached volume
//! or a volume in a transient status fails without a paid server. After the
//! server POST, any failure detaches an already attached cache and destroys the
//! server. A failed detach retains the server instead, because deleting a
//! server through the API also deletes its attached volumes.

use super::{
    super::{
        ScalewayBackend,
        ScalewayBackendError,
        volume::{ExpectedVolumeMap, expected_map},
    },
    InstanceSnapshot,
};
use crate::{
    backend::{Backend, InstanceHandle, InstanceRequest},
    scaleway::types::{Action, Zone},
};

impl ScalewayBackend {
    /// Creates an instance from an already resolved image identifier.
    ///
    /// This is the seam the loopback tests use: image resolution needs the SDK
    /// and the production host, while everything after it is ordinary HTTP.
    ///
    /// # Errors
    ///
    /// Returns a validation error, a preflight volume error, the provider's
    /// create or attach error, or a rollback error that wraps the original.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let handle = backend.create_with_image_id(&request, "image-1").await?;
    /// assert_eq!(handle.zone, request.zone);
    /// ```
    pub(in crate::scaleway) async fn create_with_image_id(
        &self,
        request: &InstanceRequest,
        image_id: &str,
    ) -> Result<InstanceHandle, ScalewayBackendError> {
        request.validate()?;
        let volume_id = request.volume_id.as_deref();

        if let Some(id) = volume_id {
            let target = PreflightTarget {
                volume_id: id,
                zone: &request.zone,
                project_id: &request.project_id,
            };
            self.preflight_cache_volume(&target).await?;
        }

        let server = self.create_instance_stopped(request, image_id).await?;
        let handle = InstanceHandle {
            id: server.id.clone(),
            zone: request.zone.clone(),
        };
        let expected = expected_map(&server.volumes.volumes);

        let attached_cache = match self
            .attach_configured_cache(&handle, volume_id, &expected)
            .await
        {
            Ok(cache) => cache,
            Err(failure) => {
                return Err(self
                    .rollback_after_create(&handle, failure.cache, failure.error)
                    .await);
            }
        };

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
        if let Err(error) = self.power_on_if_needed(&zone, &snapshot).await {
            return Err(self
                .rollback_after_create(&handle, attached_cache, error)
                .await);
        }

        Ok(handle)
    }

    /// Attaches the configured cache and reports whether it may be attached.
    ///
    /// A rejected attach request did not attach anything. Any other failure
    /// after a successful POST may have attached the volume, so the caller
    /// must treat the cache as present during rollback and detach it before
    /// the server is destroyed.
    async fn attach_configured_cache<'a>(
        &self,
        handle: &InstanceHandle,
        volume_id: Option<&'a str>,
        expected: &ExpectedVolumeMap,
    ) -> Result<Option<AttachedCache<'a>>, PartialCreateFailure<'a>> {
        let Some(id) = volume_id else {
            return Ok(None);
        };

        let Some(root) = expected.get("0") else {
            return Err(PartialCreateFailure {
                cache: None,
                error: ScalewayBackendError::VolumeNotFound {
                    volume_id: String::from("0"),
                    zone: handle.zone.clone(),
                },
            });
        };

        if let Err(error) = Self::validate_cache_volume_id(id, &root.id, handle) {
            return Err(PartialCreateFailure { cache: None, error });
        }

        match self.attach_volume(handle, id, expected).await {
            Ok(()) => Ok(Some(AttachedCache { volume_id: id })),
            Err(error) => {
                let cache = (!matches!(error, ScalewayBackendError::VolumeAttachmentFailed { .. }))
                    .then_some(AttachedCache { volume_id: id });
                Err(PartialCreateFailure { cache, error })
            }
        }
    }

    /// Rejects a configured cache volume before any server is created.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeNotInZone`] for a lookup failure,
    /// [`ScalewayBackendError::VolumeProjectMismatch`],
    /// [`ScalewayBackendError::VolumeLegacy`],
    /// [`ScalewayBackendError::VolumeAlreadyAttached`] or
    /// [`ScalewayBackendError::VolumeUnavailable`] for the other refusals.
    async fn preflight_cache_volume(
        &self,
        target: &PreflightTarget<'_>,
    ) -> Result<(), ScalewayBackendError> {
        let state = self.get_volume(target.zone, target.volume_id).await?;

        if state.zone != target.zone {
            return Err(ScalewayBackendError::VolumeNotInZone {
                volume_id: target.volume_id.to_owned(),
                zone: target.zone.to_owned(),
            });
        }
        // A blank project ID is outside the documented response, so it must not
        // bypass the ownership check.
        if state.project_id.trim() != target.project_id.trim() {
            return Err(ScalewayBackendError::VolumeProjectMismatch {
                volume_id: target.volume_id.to_owned(),
                expected_project_id: target.project_id.to_owned(),
                actual_project_id: state.project_id.clone(),
            });
        }
        if state.is_legacy() {
            return Err(ScalewayBackendError::VolumeLegacy {
                volume_id: target.volume_id.to_owned(),
            });
        }
        check_attached(&state, target)?;
        if !state.status.is_available() {
            return Err(ScalewayBackendError::VolumeUnavailable {
                volume_id: target.volume_id.to_owned(),
                status: state.status.to_string(),
                guidance: String::from("wait until it is 'available' before using it as a cache"),
            });
        }
        Ok(())
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
    /// // Two different volumes pass, so the attachment may proceed.
    /// ScalewayBackend::validate_cache_volume_id("vol-cache", "vol-1", &handle)?;
    /// ```
    pub(in crate::scaleway) fn validate_cache_volume_id(
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

    /// Compensates for a failure after the server POST.
    ///
    /// When the cache was attached, the server is stopped first and the volume
    /// detached; a failure in either step retains the server and wraps the
    /// original error, because destroying it would delete the cache. When no
    /// cache is attached, the server is destroyed and the original error is
    /// returned unchanged.
    async fn rollback_after_create(
        &self,
        handle: &InstanceHandle,
        cache: Option<AttachedCache<'_>>,
        original: ScalewayBackendError,
    ) -> ScalewayBackendError {
        if let Some(attached) = cache {
            let id = attached.volume_id;
            if let Err(error) = self.power_off_if_needed(handle).await {
                return ScalewayBackendError::RollbackFailed {
                    original: Box::new(original),
                    cleanup: format!(
                        "could not stop server {} before detaching volume {id}: {error}; the server was retained",
                        handle.id
                    ),
                };
            }
            if let Err(error) = self.detach_volume(handle, id).await {
                return ScalewayBackendError::RollbackFailed {
                    original: Box::new(original),
                    cleanup: format!(
                        "could not detach volume {id} from server {}: {error}; the server was retained to protect the cache",
                        handle.id
                    ),
                };
            }
        }

        match Backend::destroy(self, handle.clone()).await {
            Ok(()) => original,
            Err(error) => ScalewayBackendError::RollbackFailed {
                original: Box::new(original),
                cleanup: format!("could not destroy server {}: {error}", handle.id),
            },
        }
    }
}

/// Cache volume known to be attached during a failed create.
#[derive(Clone, Copy)]
struct AttachedCache<'a> {
    /// Volume that must be detached before the server can be destroyed.
    volume_id: &'a str,
}

/// Cache volume and the location it must match before any server exists.
struct PreflightTarget<'a> {
    /// Configured volume identifier.
    volume_id: &'a str,
    /// Availability Zone the volume must belong to.
    zone: &'a str,
    /// Project the volume must belong to.
    project_id: &'a str,
}

/// A post-create failure and any cache that rollback must detach first.
struct PartialCreateFailure<'a> {
    /// Cache that may be attached, or `None` when the attach never happened.
    cache: Option<AttachedCache<'a>>,
    /// Original error to report once cleanup finishes.
    error: ScalewayBackendError,
}

/// Rejects a volume that is attached or carries any reference.
fn check_attached(
    state: &crate::volume::VolumeState,
    target: &PreflightTarget<'_>,
) -> Result<(), ScalewayBackendError> {
    if !state.status.is_in_use() && state.references.is_empty() {
        return Ok(());
    }
    let references = if state.references.is_empty() {
        String::from("provider reports status in_use")
    } else {
        state
            .references
            .iter()
            .map(crate::volume::VolumeReference::describe)
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(ScalewayBackendError::VolumeAlreadyAttached {
        volume_id: target.volume_id.to_owned(),
        references,
        recovery: detach_hint(state, target),
    })
}

/// Builds a runnable command, or a lookup instruction, for the attachment.
fn detach_hint(state: &crate::volume::VolumeState, target: &PreflightTarget<'_>) -> String {
    if let Some(reference) = state
        .references
        .iter()
        .find(|reference| reference.resource_type.contains("server"))
    {
        return format!(
            "detach it with `scw instance server detach-volume server-id={} volume-id={} zone={}`",
            reference.resource_id, target.volume_id, target.zone
        );
    }
    state.references.first().map_or_else(
        || {
            format!(
                "find the attaching resource with `scw block volume get {} zone={}`",
                target.volume_id, target.zone
            )
        },
        |reference| {
            format!(
                "volume {} is attached to {}:{} in zone {}; inspect that resource to detach it",
                target.volume_id, reference.resource_type, reference.resource_id, target.zone
            )
        },
    )
}
