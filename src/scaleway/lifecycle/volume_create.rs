//! Block Storage volume creation helpers for the Scaleway backend.

use serde::{Deserialize, Serialize};

use crate::volume::{VolumeHandle, VolumeRequest};

use super::super::{ScalewayBackend, ScalewayBackendError};

const VOLUME_TYPE_BLOCK: &str = "b_ssd";

#[derive(Serialize)]
struct CreateVolumeRequest {
    name: String,
    size: u64,
    volume_type: String,
    project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    organization: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct CreateVolumeResponse {
    volume: ScalewayVolume,
}

#[derive(Deserialize)]
struct ScalewayVolume {
    id: String,
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
        let url = format!(
            "{}/zones/{}/volumes",
            super::SCALEWAY_INSTANCE_API_BASE,
            request.zone
        );
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
