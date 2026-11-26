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

use serde::Deserialize;

use super::super::{ScalewayBackend, ScalewayBackendError};
use crate::{
    backend::{InstanceHandle, InstanceRequest},
    boot_volume::{
        BootVolumeAllocation,
        BootVolumeMismatch,
        BootVolumePlan,
        BootVolumeType,
        RootVolume,
    },
    scaleway::lifecycle::{
        boot_plan::{ImageFacts, resolve_boot_volume},
        create::CreatedServer,
    },
    teardown::append_teardown_note,
};

/// The subset of a Block Storage volume that root capture reads.
///
/// # Examples
///
/// ```ignore
/// let volume: BlockVolume = serde_json::from_str(
///     r#"{"id":"vol-1","type":"sbs_5k","size":20000000000,"status":"available"}"#,
/// )?;
/// assert_eq!(volume.size, 20_000_000_000);
/// ```
#[derive(Debug, Deserialize)]
pub(in crate::scaleway) struct BlockVolume {
    /// Provider identifier of the volume.
    id: String,
    /// Capacity in bytes.
    size: u64,
}

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

        let plan = resolve_boot_volume(
            &request.instance_type,
            &request.boot_volume,
            &product,
            ImageFacts {
                size_bytes: image.size_bytes,
                storage: image.storage,
                volume_type: &image.volume_type,
            },
            &request.architecture,
        )?;

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
    /// identifies the instance, so the caller can destroy it. The cache volume
    /// is deliberately not detached first: a failed attachment may or may not
    /// have taken effect, and deleting the server is the guaranteed cleanup
    /// for an ephemeral instance.
    ///
    /// # Errors
    ///
    /// Returns the first failing step's error paired with the handle.
    pub(in crate::scaleway) async fn finish_created_server(
        &self,
        request: &InstanceRequest,
        server: &CreatedServer,
        plan: &BootVolumePlan,
        mut handle: InstanceHandle,
    ) -> Result<InstanceHandle, (ScalewayBackendError, InstanceHandle)> {
        let allocation = self
            .verify_root_volume(&server.id, &request.zone, server, plan)
            .await
            .map_err(|error| (error, handle.clone()))?;
        handle.boot_volume = Some(allocation);

        if let Some(cache_volume_id) = request.volume_id.as_deref() {
            let root = handle
                .boot_volume
                .as_ref()
                .map(|allocation| allocation.root.clone())
                .ok_or_else(|| {
                    (
                        ScalewayBackendError::VolumeNotFound {
                            volume_id: String::from("0"),
                            zone: request.zone.clone(),
                        },
                        handle.clone(),
                    )
                })?;
            Self::validate_cache_volume_id(cache_volume_id, &root.id, &handle)
                .map_err(|error| (error, handle.clone()))?;
            self.attach_volume(&handle, cache_volume_id, &root)
                .await
                .map_err(|error| (error, handle.clone()))?;
        }

        let zone = crate::scaleway::types::Zone::from(request.zone.as_str());
        self.power_on_if_needed(&zone, &server.snapshot_ref())
            .await
            .map_err(|error| (error, handle.clone()))?;

        Ok(handle)
    }

    /// Destroys the instance after a failed create and returns the original
    /// error, with the teardown outcome appended when that also failed.
    ///
    /// The original error is preserved as the source, so a caller can still
    /// classify it while the message carries both outcomes.
    ///
    /// For example, a root-volume mismatch whose cleanup succeeded returns the
    /// mismatch unchanged, while one whose cleanup failed appends
    /// `(teardown also failed: ...)`.
    pub(in crate::scaleway) async fn clean_up_after_failed_create(
        &self,
        handle: &InstanceHandle,
        error: ScalewayBackendError,
    ) -> ScalewayBackendError {
        let teardown_error = self.destroy_for_cleanup(handle).await.err();
        let message = append_teardown_note(error.to_string(), teardown_error.as_ref());
        ScalewayBackendError::CreateCleanupFailed {
            message,
            source: Box::new(error),
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

    /// Reads the volume the created instance boots from.
    ///
    /// Local volumes report their size and type in the create response. Block
    /// Storage volumes are read from the Block Storage API by identifier,
    /// because the Instances API does not report an attached SBS volume's
    /// size reliably.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeNotFound`] when the response
    /// carries no bootable volume, an
    /// [`ScalewayBackendError::BootVolumeInvalid`] when the reported type is
    /// not one this backend models, or
    /// [`ScalewayBackendError::BlockVolumeReadFailed`] when the Block Storage
    /// read fails.
    ///
    /// For example, a local 10 GB root yields that identifier, `l_ssd`, and
    /// 10_000_000_000 bytes.
    pub(in crate::scaleway) async fn capture_root_volume(
        &self,
        instance_id: &str,
        zone: &str,
        server: &CreatedServer,
    ) -> Result<RootVolume, ScalewayBackendError> {
        let (_, volume) = server.boot_volume().ok_or_else(|| {
            ScalewayBackendError::BootVolumeMismatch(BootVolumeMismatch::Missing {
                instance_id: instance_id.to_owned(),
            })
        })?;

        let volume_type = super::create::reported_volume_type(volume).ok_or_else(|| {
            ScalewayBackendError::BootVolumeInvalid {
                message: format!(
                    "instance {instance_id} root volume {} reports unknown type {:?}",
                    volume.id, volume.volume_type
                ),
            }
        })?;

        match volume_type {
            BootVolumeType::Local => {
                let size_bytes =
                    volume
                        .size
                        .ok_or_else(|| ScalewayBackendError::BootVolumeInvalid {
                            message: format!(
                                "instance {instance_id} local root volume {} reports no size",
                                volume.id
                            ),
                        })?;
                Ok(RootVolume {
                    id: volume.id.clone(),
                    volume_type,
                    size_bytes,
                })
            }
            BootVolumeType::Sbs => {
                let block = self.read_block_volume(zone, &volume.id).await?;
                Ok(RootVolume {
                    id: block.id,
                    volume_type,
                    size_bytes: block.size,
                })
            }
        }
    }

    /// Reads one Block Storage volume by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::BlockVolumeReadFailed`] for a provider
    /// rejection and [`ScalewayBackendError::Provider`] for a transport
    /// failure, matching how the other HTTP wrappers classify.
    ///
    /// For example, an available 20 GB volume yields its identifier and size.
    async fn read_block_volume(
        &self,
        zone: &str,
        volume_id: &str,
    ) -> Result<BlockVolume, ScalewayBackendError> {
        let url = format!("{}/zones/{zone}/volumes/{volume_id}", self.block_api_base);
        let response = super::HTTP_CLIENT
            .get(&url)
            .header("X-Auth-Token", &self.config.secret_key)
            .send()
            .await
            .map_err(|err| ScalewayBackendError::Provider {
                message: err.to_string(),
            })?;

        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|err| ScalewayBackendError::Provider {
                message: err.to_string(),
            })?;

        if status.is_success() {
            return serde_json::from_slice(&body).map_err(|err| {
                ScalewayBackendError::BlockVolumeReadFailed {
                    volume_id: volume_id.to_owned(),
                    zone: zone.to_owned(),
                    message: err.to_string(),
                }
            });
        }

        Err(ScalewayBackendError::BlockVolumeReadFailed {
            volume_id: volume_id.to_owned(),
            zone: zone.to_owned(),
            message: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    /// Verifies the reported root volume against `plan` and records it.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::BootVolumeMismatch`] when the reported
    /// type or size differs from the plan, so a silent provider substitution
    /// fails the run instead of changing what the user bought.
    ///
    /// For example, a plan for a 20 GB `l_ssd` answered by a 10 GB volume
    /// fails naming both sizes.
    pub(in crate::scaleway) async fn verify_root_volume(
        &self,
        instance_id: &str,
        zone: &str,
        server: &CreatedServer,
        plan: &BootVolumePlan,
    ) -> Result<BootVolumeAllocation, ScalewayBackendError> {
        let root = self.capture_root_volume(instance_id, zone, server).await?;
        BootVolumeAllocation::verify(instance_id, plan, root)
            .map_err(ScalewayBackendError::BootVolumeMismatch)
    }
}
