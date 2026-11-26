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
    /// Returns [`ScalewayBackendError::VolumeAttachmentFailed`] only for a 4xx
    /// rejection, which proves nothing was attached. Any other non-success
    /// status, an unparsable body or a map that violates the contract means the
    /// volume may be attached, so those return
    /// [`ScalewayBackendError::VolumeAttachmentUnverified`] and the caller must
    /// detach before destroying the server. Lookup and timeout errors from the
    /// status wait propagate unchanged.
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
            // A 4xx rejection proves the request was refused before it took
            // effect. A 5xx or gateway status does not: the provider may have
            // attached the volume before the failure, so that case must stay
            // detachable.
            return Err(if status.is_client_error() {
                ScalewayBackendError::VolumeAttachmentFailed {
                    volume_id: volume_id.to_owned(),
                    instance_id: handle.id.clone(),
                    message: body,
                }
            } else {
                ScalewayBackendError::VolumeAttachmentUnverified {
                    volume_id: volume_id.to_owned(),
                    instance_id: handle.id.clone(),
                    message: format!("attach-volume returned {status}: {body}"),
                }
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
