//! Block Storage volume creation for the Scaleway backend.
//!
//! Scaleway retired new Block Storage volume creation through the Instances API
//! on 30 April 2025. Creation therefore targets the Block Storage v1 API, whose
//! create response is the unwrapped `Volume` object. Before creating anything
//! the backend checks that the requested performance tier is in stock for the
//! zone, so an unsupported tier fails without a provider-side allocation.

use serde::Serialize;

use super::super::{
    ScalewayBackend,
    ScalewayBackendError,
    types::{SbsVolume, VolumeTypesResponse},
};
use crate::volume::{VolumeHandle, VolumeRequest};

/// JSON body for the Block Storage create-volume call.
///
/// # Examples
///
/// ```ignore
/// let body = CreateVolumeRequest {
///     name: String::from("mriya-cache"),
///     project_id: String::from("proj-1"),
///     perf_iops: 5_000,
///     from_empty: FromEmpty { size: 20_000_000_000 },
///     tags: vec![String::from("mriya")],
/// };
/// let json = serde_json::to_value(&body)?;
/// assert_eq!(json["perf_iops"], 5_000);
/// assert_eq!(json["from_empty"]["size"], 20_000_000_000u64);
/// ```
#[derive(Serialize)]
struct CreateVolumeRequest<'a> {
    /// Volume name as supplied by the caller.
    name: &'a str,
    /// Project that will own the volume.
    project_id: &'a str,
    /// Requested IOPS, which selects the storage tier.
    perf_iops: u32,
    /// Size of a brand-new volume.
    from_empty: FromEmpty,
    /// Tags for identifying cache volumes; omitted from the body when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

/// Size selector for a volume created from scratch.
#[derive(Serialize)]
struct FromEmpty {
    /// Volume size in decimal bytes.
    size: u64,
}

impl ScalewayBackend {
    /// Creates a new Block Storage volume.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeRequest`] when the request is not
    /// creation-ready, [`ScalewayBackendError::VolumeTierUnavailable`] when the
    /// zone does not stock the requested tier, and
    /// [`ScalewayBackendError::VolumeCreateFailed`] when the provider rejects
    /// the create request.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // A successful create returns the provider's identifier for the volume.
    /// let handle = backend.create_volume(&request).await?;
    /// assert_eq!(handle.id, "11111111-1111-1111-1111-111111111111");
    ///
    /// // A rejected create names the volume and the provider's message.
    /// let err = backend.create_volume(&request).await.unwrap_err();
    /// assert!(matches!(err, ScalewayBackendError::VolumeCreateFailed { .. }));
    /// ```
    pub(in crate::scaleway) async fn create_volume(
        &self,
        request: &VolumeRequest,
    ) -> Result<VolumeHandle, ScalewayBackendError> {
        request.validate()?;
        self.ensure_volume_tier_available(&request.zone, request.tier)
            .await?;

        let url = format!("{}/zones/{}/volumes", self.block_api_base, request.zone);
        let payload = CreateVolumeRequest {
            name: &request.name,
            project_id: &request.project_id,
            perf_iops: request.tier.perf_iops(),
            from_empty: FromEmpty {
                size: request.size_bytes,
            },
            tags: Self::volume_tags(self.test_run_id.as_deref()),
        };

        let response = super::http::send_raw(
            super::HTTP_CLIENT
                .post(&url)
                .header("X-Auth-Token", &self.config.secret_key)
                .json(&payload),
        )
        .await?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| ScalewayBackendError::Provider {
                message: err.to_string(),
            })?;

        if status.is_success() {
            let volume: SbsVolume =
                serde_json::from_str(&body).map_err(|err| ScalewayBackendError::Provider {
                    message: err.to_string(),
                })?;
            return Ok(VolumeHandle {
                id: volume.id,
                zone: volume.zone,
            });
        }

        Err(ScalewayBackendError::VolumeCreateFailed {
            name: request.name.clone(),
            zone: request.zone.clone(),
            message: body,
        })
    }

    /// Fails unless the zone lists the requested tier.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeTierUnavailable`] when the tier is
    /// absent from the zone's volume types, and a provider error when the list
    /// cannot be read.
    async fn ensure_volume_tier_available(
        &self,
        zone: &str,
        tier: crate::volume::VolumeTier,
    ) -> Result<(), ScalewayBackendError> {
        let url = format!("{}/zones/{zone}/volume-types", self.block_api_base);
        let response = super::http::checked_response(
            super::HTTP_CLIENT
                .get(&url)
                .header("X-Auth-Token", &self.config.secret_key),
        )
        .await?;
        let listing: VolumeTypesResponse =
            response
                .json()
                .await
                .map_err(|err| ScalewayBackendError::Provider {
                    message: err.to_string(),
                })?;

        let is_listed = listing
            .volume_types
            .iter()
            .any(|candidate| candidate.volume_type == tier.as_str());
        if is_listed {
            return Ok(());
        }

        Err(ScalewayBackendError::VolumeTierUnavailable {
            tier,
            zone: zone.to_owned(),
        })
    }
}
