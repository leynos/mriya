//! Root-volume capture and verification.
//!
//! After a server exists, Mriya reads the root volume the provider actually
//! created, checks it against the plan, and records it on the handle. This is
//! the point where "what was requested" becomes "what was bought".
//!
//! A local volume's size and type arrive in the create response. A Block
//! Storage volume's do not, reliably, so those are read from the Block Storage
//! API instead.

use serde::Deserialize;

use super::super::{ScalewayBackend, ScalewayBackendError};
use crate::{
    boot_volume::{
        BootVolumeAllocation,
        BootVolumeMismatch,
        BootVolumePlan,
        BootVolumeType,
        RootVolume,
    },
    scaleway::lifecycle::create::CreatedServer,
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

impl ScalewayBackend {
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
    /// `10_000_000_000` bytes.
    pub(in crate::scaleway) async fn capture_root_volume(
        &self,
        zone: &str,
        server: &CreatedServer,
    ) -> Result<RootVolume, ScalewayBackendError> {
        let instance_id = server.id.as_str();
        let (_, volume) = server.boot_volume().ok_or_else(|| {
            ScalewayBackendError::BootVolumeMismatch(Box::new(BootVolumeMismatch::Missing {
                instance_id: instance_id.to_owned(),
            }))
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
        server: &CreatedServer,
        zone: &str,
        plan: &BootVolumePlan,
    ) -> Result<BootVolumeAllocation, ScalewayBackendError> {
        let root = self.capture_root_volume(zone, server).await?;
        BootVolumeAllocation::verify(&server.id, plan, root).map_err(ScalewayBackendError::from)
    }
}
