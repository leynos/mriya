//! Root-volume capture and post-create verification.
//!
//! After a server exists, Mriya reads the root volume the provider actually
//! created, checks it against the plan, and records it on the handle. This is
//! the point where "what was requested" becomes "what was bought": the plan
//! says what the request asked for, and the capture says what the account is
//! now paying for.
//!
//! A local volume's size and type arrive in the create response. A Block
//! Storage volume's do not, reliably, so those are read from the Block Storage
//! API instead.

use super::super::{ScalewayBackend, ScalewayBackendError};
use crate::{
    backend::{InstanceHandle, InstanceRequest},
    boot_volume::BootVolumePlan,
    scaleway::lifecycle::{
        boot_plan::{ImageFacts, Resolution, resolve_boot_volume},
        create::CreatedServer,
    },
    teardown::append_teardown_note,
};

/// A resolved boot plan together with the image it was resolved against.
///
/// Pairing them is what keeps `create` to a single reading of the catalogue:
/// resolving the plan from one image and posting a different one would make
/// the verified root volume disagree with what was requested.
#[derive(Clone, Debug)]
pub(in crate::scaleway) struct ResolvedBoot {
    /// The plan the create request will send.
    pub(in crate::scaleway) plan: BootVolumePlan,
    /// Provider identifier of the image the plan was resolved against.
    pub(in crate::scaleway) image_id: String,
}

/// A create that has produced a server but has not finished.
///
/// Grouping these keeps the follow-up signature short and makes the inputs a
/// caller must supply explicit.
#[derive(Debug)]
pub(in crate::scaleway) struct CreatedInstance<'a> {
    /// The request that produced the server.
    pub(in crate::scaleway) request: &'a InstanceRequest,
    /// The server the provider reported.
    pub(in crate::scaleway) server: &'a CreatedServer,
    /// The plan the create request sent.
    pub(in crate::scaleway) plan: &'a BootVolumePlan,
    /// The handle identifying the server, with no boot record yet.
    pub(in crate::scaleway) handle: InstanceHandle,
}

/// Why a created instance could not be finished, keeping the handle so the
/// caller can still destroy the server.
#[derive(Debug)]
pub(in crate::scaleway) struct CreateFailure {
    /// The original failure.
    pub(in crate::scaleway) error: ScalewayBackendError,
    /// A handle identifying the server to clean up.
    pub(in crate::scaleway) handle: InstanceHandle,
    /// True when the server must be kept because it may hold the cache volume.
    ///
    /// Deleting a server through the API deletes its attached volumes, so a
    /// failure that leaves the cache attached, or that cannot prove it is
    /// detached, must not be followed by a delete.
    pub(in crate::scaleway) keep_server: bool,
}

impl ScalewayBackend {
    /// Resolves the boot plan and the image identifier in one pass.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::resolve_boot_plan`].
    ///
    /// For example, a valid 20 GB local request yields a matching plan and the
    /// newest compatible image's identifier.
    pub(in crate::scaleway) async fn resolve_plan_and_image(
        &self,
        request: &InstanceRequest,
    ) -> Result<ResolvedBoot, ScalewayBackendError> {
        request.validate()?;
        let product = self
            .fetch_server_type(&request.zone, &request.instance_type)
            .await?;
        let image = self.resolve_image_id(request).await?;

        let plan = resolve_boot_volume(Resolution {
            instance_type: &request.instance_type,
            request: &request.boot_volume,
            product: &product,
            image: ImageFacts {
                size_bytes: image.size_bytes,
                storage: image.storage,
                volume_type: &image.volume_type,
            },
            requested_architecture: &request.architecture,
        })?;

        Ok(ResolvedBoot {
            plan,
            image_id: image.id,
        })
    }

    /// Resolves the boot-volume plan without creating anything.
    ///
    /// This is the same resolution `create` performs, so a preflight that
    /// succeeds and a create that follows cannot disagree about the disk.
    ///
    /// # Errors
    ///
    /// Returns the first resolution rule that refuses the request, a provider
    /// error when the catalogue or image lookup fails, or
    /// [`ScalewayBackendError::ImageNotFound`] when the label matches no image.
    ///
    /// For example, a 20 GB local request against a type with no local
    /// storage fails before any server exists.
    pub(in crate::scaleway) async fn resolve_boot_plan(
        &self,
        request: &InstanceRequest,
    ) -> Result<BootVolumePlan, ScalewayBackendError> {
        Ok(self.resolve_plan_and_image(request).await?.plan)
    }

    /// Completes a create: verifies the root, attaches the cache volume, and
    /// powers the instance on.
    ///
    /// On failure it returns the reason together with a handle that still
    /// identifies the instance, so the caller can destroy it.
    ///
    /// The steps are ordered so cleanup stays safe: verification runs before
    /// the cache is attached, so a verification failure can destroy the server
    /// with nothing but the ephemeral boot disk on it. A power-on failure
    /// happens with the cache attached, and deleting the server would delete
    /// that cache, so the cache is detached first; a failed detach leaves the
    /// server running instead.
    ///
    /// # Errors
    ///
    /// Returns the first failing step's error paired with the handle.
    pub(in crate::scaleway) async fn finish_created_server(
        &self,
        created: CreatedInstance<'_>,
    ) -> Result<InstanceHandle, Box<CreateFailure>> {
        let CreatedInstance {
            request,
            server,
            plan,
            mut handle,
        } = created;

        let fail = |error: ScalewayBackendError, failed_handle: &InstanceHandle| {
            Box::new(CreateFailure {
                error,
                handle: failed_handle.clone(),
                keep_server: false,
            })
        };

        let allocation = self
            .verify_root_volume(server, &request.zone, plan)
            .await
            .map_err(|error| fail(error, &handle))?;
        handle.boot_volume = Some(allocation);

        if let Some(cache_volume_id) = request.volume_id.as_deref() {
            let root = handle
                .boot_volume
                .as_ref()
                .map(|record| record.root.clone())
                .ok_or_else(|| {
                    fail(
                        ScalewayBackendError::VolumeNotFound {
                            volume_id: String::from("0"),
                            zone: request.zone.clone(),
                        },
                        &handle,
                    )
                })?;
            Self::validate_cache_volume_id(cache_volume_id, &root.id, &handle)
                .map_err(|error| fail(error, &handle))?;
            if let Err(attach_error) = self.attach_volume(&handle, cache_volume_id, &root).await {
                return Err(self
                    .classify_attachment_failure(&handle, cache_volume_id, attach_error)
                    .await);
            }
        }

        let zone = crate::scaleway::types::Zone::from(request.zone.as_str());
        if let Err(power_error) = self.power_on_if_needed(&zone, &server.snapshot_ref()).await {
            // Deleting a server through the API also deletes its attached
            // volumes. With a cache volume attached, destroying the server
            // would destroy the user's cache, so the cache is detached first.
            // A failed detach leaves the server running rather than trading a
            // billing instance for someone's data.
            if let Some(cache_volume_id) = request.volume_id.as_deref()
                && let Err(detach_error) = self.detach_volume(&handle, cache_volume_id).await
            {
                let message = format!(
                    "{power_error} (the cache volume {cache_volume_id} could not be \
                     detached: {detach_error}; the server was left running rather \
                     than risk deleting the cache)"
                );
                return Err(Box::new(CreateFailure {
                    error: ScalewayBackendError::CreateCleanupFailed {
                        message,
                        source: Box::new(power_error),
                    },
                    handle,
                    keep_server: true,
                }));
            }
            return Err(fail(power_error, &handle));
        }

        Ok(handle)
    }

    /// Decides whether the server may be destroyed after an attachment
    /// failure.
    ///
    /// A rejected response means the PATCH did not apply, so deleting the
    /// server is safe. A transport failure is ambiguous — the provider may have
    /// applied the change before the error reached the client — so the instance
    /// is read back to settle it. A read that also fails keeps the server,
    /// because an unproven detachment must not be followed by a delete that
    /// would take the cache with it.
    pub(in crate::scaleway) async fn classify_attachment_failure(
        &self,
        handle: &InstanceHandle,
        cache_volume_id: &str,
        attach_error: ScalewayBackendError,
    ) -> Box<CreateFailure> {
        if !matches!(attach_error, ScalewayBackendError::Provider { .. }) {
            return Box::new(CreateFailure {
                error: attach_error,
                handle: handle.clone(),
                keep_server: false,
            });
        }

        match self.is_volume_attached(handle, cache_volume_id).await {
            Ok(false) => Box::new(CreateFailure {
                error: attach_error,
                handle: handle.clone(),
                keep_server: false,
            }),
            Ok(true) => Box::new(CreateFailure {
                error: ScalewayBackendError::CreateCleanupFailed {
                    message: format!(
                        "{attach_error} (the cache volume {cache_volume_id} is attached \
                         despite the error; the server was left running rather than risk \
                         deleting the cache)"
                    ),
                    source: Box::new(attach_error),
                },
                handle: handle.clone(),
                keep_server: true,
            }),
            Err(read_error) => Box::new(CreateFailure {
                error: ScalewayBackendError::CreateCleanupFailed {
                    message: format!(
                        "{attach_error} (the cache volume {cache_volume_id} may be attached, and \
                         checking failed: {read_error}; the server was left running rather than \
                         risk deleting the cache)"
                    ),
                    source: Box::new(attach_error),
                },
                handle: handle.clone(),
                keep_server: true,
            }),
        }
    }

    /// Reports whether `volume_id` is attached to the instance.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the instance cannot be read.
    pub(in crate::scaleway) async fn is_volume_attached(
        &self,
        handle: &InstanceHandle,
        volume_id: &str,
    ) -> Result<bool, ScalewayBackendError> {
        let server = self.get_instance(handle).await?;
        Ok(server.volumes.values().any(|volume| volume.id == volume_id))
    }

    /// Destroys the instance after a failed create and returns the original
    /// error, with the teardown outcome appended only when that also failed.
    ///
    /// The original error is preserved as the source of the combined error, so
    /// a caller can still classify it while the message carries both outcomes.
    ///
    /// For example, a root-volume mismatch whose cleanup succeeded returns the
    /// mismatch unchanged, while one whose cleanup failed appends
    /// `(teardown also failed: ...)`.
    pub(in crate::scaleway) async fn clean_up_after_failed_create(
        &self,
        failure: Box<CreateFailure>,
    ) -> ScalewayBackendError {
        let CreateFailure {
            error,
            handle,
            keep_server,
        } = *failure;
        if keep_server {
            return ScalewayBackendError::CreateCleanupFailed {
                message: format!(
                    "{error} (the server was left running because the cache volume may \
                     still be attached)"
                ),
                source: Box::new(error),
            };
        }
        match self.destroy_for_cleanup(&handle).await {
            Ok(()) => error,
            Err(teardown_error) => ScalewayBackendError::CreateCleanupFailed {
                message: append_teardown_note(error.to_string(), Some(&teardown_error)),
                source: Box::new(error),
            },
        }
    }

    /// Deletes the instance without waiting for its disappearance.
    ///
    /// A create that failed part-way has no readiness contract to honour, so
    /// this makes the single delete call rather than the full `destroy`
    /// sequence. The janitor still sweeps anything a failed call leaves.
    async fn destroy_for_cleanup(
        &self,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        self.delete_instance(handle).await
    }
}
