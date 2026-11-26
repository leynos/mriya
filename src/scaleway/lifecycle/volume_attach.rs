//! Volume attachment through the dedicated Instances endpoint.
//!
//! The request names one volume and its provider type, so the provider keeps
//! the rest of the server's volume map. After the response is parsed, the
//! returned map is checked against the map from create time: the root entry and
//! every unrelated entry must survive, and the cache entry must be present with
//! `volume_type: sbs_volume`. Creation then waits for the Block Storage status
//! `in_use` so the caller never assumes a successful attach too early.

use super::super::{
    ScalewayBackend,
    ScalewayBackendError,
    volume::{
        AttachServerVolumeRequest,
        ExpectedVolumeMap,
        ServerVolumesResponse,
        verify_attached,
    },
};
use crate::{backend::InstanceHandle, volume::VolumeStatus};

impl ScalewayBackend {
    /// Attaches a volume to a stopped instance and waits for `in_use`.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeAttachmentFailed`] when the API
    /// rejects the request or the returned map violates the contract, and any
    /// lookup or timeout error from the status wait.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let expected = expected_map(&server.volumes.volumes);
    /// backend.attach_volume(&handle, "vol-cache", &expected).await?;
    /// ```
    pub(in crate::scaleway) async fn attach_volume(
        &self,
        handle: &InstanceHandle,
        volume_id: &str,
        expected: &ExpectedVolumeMap,
    ) -> Result<(), ScalewayBackendError> {
        let url = format!(
            "{}/zones/{}/servers/{}/attach-volume",
            self.instance_api_base, handle.zone, handle.id
        );
        let payload = AttachServerVolumeRequest::new(volume_id);

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

        if !status.is_success() {
            return Err(ScalewayBackendError::VolumeAttachmentFailed {
                volume_id: volume_id.to_owned(),
                instance_id: handle.id.clone(),
                message: body,
            });
        }

        let parsed: ServerVolumesResponse = serde_json::from_str(&body).map_err(|err| {
            ScalewayBackendError::VolumeAttachmentUnverified {
                volume_id: volume_id.to_owned(),
                instance_id: handle.id.clone(),
                message: format!("attach-volume response was not a server object: {err}"),
            }
        })?;
        verify_attached(&parsed.server.volumes, volume_id, expected).map_err(|message| {
            ScalewayBackendError::VolumeAttachmentUnverified {
                volume_id: volume_id.to_owned(),
                instance_id: handle.id.clone(),
                message,
            }
        })?;

        self.wait_for_volume_status(&handle.zone, volume_id, VolumeStatus::InUse)
            .await?;
        Ok(())
    }
}
