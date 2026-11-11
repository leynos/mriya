//! Block Storage volume creation helpers for the Scaleway backend.

use serde::{Deserialize, Serialize};

use super::super::{ScalewayBackend, ScalewayBackendError};
use crate::volume::{VolumeHandle, VolumeRequest};

/// Volume type requested for cache volumes (Scaleway Block Storage SSD).
const VOLUME_TYPE_BLOCK: &str = "b_ssd";

/// JSON body for the create-volume call.
#[derive(Serialize)]
struct CreateVolumeRequest {
    /// Volume name as supplied by the caller.
    name: String,
    /// Volume size in bytes, taken from `VolumeRequest::size_bytes`.
    size: u64,
    /// Provider volume type; always `VOLUME_TYPE_BLOCK` here.
    volume_type: String,
    /// Project that will own the volume.
    project: String,
    /// Optional organization identifier, omitted from the body when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    organization: Option<String>,
    /// Tags for identifying cache volumes; omitted from the body when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

/// Wrapper for the volume object returned by a successful create call.
#[derive(Deserialize)]
struct CreateVolumeResponse {
    /// The created volume as reported by the provider.
    volume: ScalewayVolume,
}

/// The subset of the provider's volume object that mriya reads.
#[derive(Deserialize)]
struct ScalewayVolume {
    /// Provider identifier of the volume.
    id: String,
    /// Zone in which the provider placed the volume.
    zone: String,
}

impl ScalewayBackend {
    /// Creates a new Block Storage volume.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeCreateFailed`] when the provider
    /// rejects the creation request.
    pub(in crate::scaleway) async fn create_volume(
        &self,
        request: &VolumeRequest,
    ) -> Result<VolumeHandle, ScalewayBackendError> {
        let url = format!("{}/zones/{}/volumes", self.instance_api_base, request.zone);
        let payload = CreateVolumeRequest {
            name: request.name.clone(),
            size: request.size_bytes,
            volume_type: String::from(VOLUME_TYPE_BLOCK),
            project: request.project_id.clone(),
            organization: request.organisation_id.clone(),
            tags: Self::volume_tags(self.test_run_id.as_deref()),
        };

        let response = super::HTTP_CLIENT
            .post(&url)
            .header("X-Auth-Token", &self.config.secret_key)
            .json(&payload)
            .timeout(super::HTTP_TIMEOUT)
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
            let parsed: CreateVolumeResponse =
                serde_json::from_slice(&body).map_err(|err| ScalewayBackendError::Provider {
                    message: err.to_string(),
                })?;
            return Ok(VolumeHandle {
                id: parsed.volume.id,
                zone: parsed.volume.zone,
            });
        }

        let message = String::from_utf8_lossy(&body).into_owned();
        Err(ScalewayBackendError::VolumeCreateFailed {
            name: request.name.clone(),
            zone: request.zone.clone(),
            message,
        })
    }
}
