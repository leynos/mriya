//! Volume detachment through the dedicated Instances endpoint.
//!
//! Detachment names one volume, so the provider keeps the root entry without a
//! client-side rebuild of the volume map. The returned map is verified (cache
//! gone, root present) and the Block Storage status must reach `available`
//! before the caller deletes the server; otherwise the provider would delete
//! the still-attached cache volume with the server.

use super::super::{
    ScalewayBackend,
    ScalewayBackendError,
    volume::{DetachServerVolumeRequest, ServerVolumesResponse, expected_map, verify_detached},
};
use crate::{backend::InstanceHandle, volume::VolumeStatus};

impl ScalewayBackend {
    /// Detaches a volume from a stopped instance and waits for `available`.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeDetachFailed`] when the API rejects
    /// the request or the returned map violates the contract, and any lookup or
    /// timeout error from the status wait.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// backend.detach_volume(&handle, "vol-cache").await?;
    /// assert_eq!(
    ///     backend.get_volume(&handle.zone, "vol-cache").await?.status,
    ///     VolumeStatus::Available
    /// );
    /// ```
    pub(in crate::scaleway) async fn detach_volume(
        &self,
        handle: &InstanceHandle,
        volume_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        let instance = self.get_instance(handle).await?;
        let mut expected = expected_map(&instance.volumes.volumes);
        expected.retain(|_, entry| entry.id != volume_id);

        let url = format!(
            "{}/zones/{}/servers/{}/detach-volume",
            self.instance_api_base, handle.zone, handle.id
        );
        let payload = DetachServerVolumeRequest { volume_id };

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
            return Err(ScalewayBackendError::VolumeDetachFailed {
                volume_id: volume_id.to_owned(),
                instance_id: handle.id.clone(),
                message: body,
            });
        }

        let parsed: ServerVolumesResponse =
            serde_json::from_str(&body).map_err(|err| ScalewayBackendError::Provider {
                message: format!("detach-volume response was not a server object: {err}"),
            })?;
        verify_detached(&parsed.server.volumes, volume_id, &expected).map_err(|message| {
            ScalewayBackendError::VolumeDetachFailed {
                volume_id: volume_id.to_owned(),
                instance_id: handle.id.clone(),
                message,
            }
        })?;

        self.wait_for_volume_status(&handle.zone, volume_id, VolumeStatus::Available)
            .await?;
        Ok(())
    }
}
