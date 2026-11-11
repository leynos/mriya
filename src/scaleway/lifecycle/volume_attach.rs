//! Block Storage volume attachment helpers for the Scaleway backend.

use std::collections::HashMap;

use super::super::{
    ScalewayBackend,
    ScalewayBackendError,
    volume::{UpdateInstanceVolumesRequest, VolumeAttachment},
};
use crate::backend::InstanceHandle;

/// Direction of a volume PATCH, used to choose the matching error variant.
#[derive(Copy, Clone, Debug)]
pub(super) enum VolumePatchAction {
    /// The PATCH adds a volume; failures map to `VolumeAttachmentFailed`.
    Attach,
    /// The PATCH removes a volume; failures map to `VolumeDetachFailed`.
    Detach,
}

impl VolumePatchAction {
    /// Builds the error variant matching this action from the failure details.
    const fn into_error(
        self,
        volume_id: String,
        instance_id: String,
        message: String,
    ) -> ScalewayBackendError {
        match self {
            Self::Attach => ScalewayBackendError::VolumeAttachmentFailed {
                volume_id,
                instance_id,
                message,
            },
            Self::Detach => ScalewayBackendError::VolumeDetachFailed {
                volume_id,
                instance_id,
                message,
            },
        }
    }
}

/// Identifies the volume and direction of a PATCH so a failure can be
/// reported as an attach or detach error.
#[derive(Copy, Clone, Debug)]
pub(super) struct VolumePatchContext<'a> {
    /// Identifier of the volume being attached or detached.
    volume_id: &'a str,
    /// Whether the PATCH attaches or detaches the volume.
    action: VolumePatchAction,
}

impl<'a> VolumePatchContext<'a> {
    /// Context for attaching `volume_id`.
    pub(super) const fn attach(volume_id: &'a str) -> Self {
        Self {
            volume_id,
            action: VolumePatchAction::Attach,
        }
    }

    /// Context for detaching `volume_id`.
    pub(super) const fn detach(volume_id: &'a str) -> Self {
        Self {
            volume_id,
            action: VolumePatchAction::Detach,
        }
    }
}

impl ScalewayBackend {
    /// Attaches a volume to a stopped instance.
    ///
    /// The volume must be in the same zone as the instance. The attachment
    /// uses a direct HTTP PATCH call since the `scaleway-rs` crate does not
    /// expose volume management in its instance builder.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeAttachmentFailed`] when the API
    /// rejects the attachment request. Returns
    /// [`ScalewayBackendError::VolumeNotFound`] when the root volume is missing
    /// from the instance snapshot used to build the attachment payload.
    pub(in crate::scaleway) async fn attach_volume(
        &self,
        handle: &InstanceHandle,
        volume_id: &str,
        root_volume_id: String,
    ) -> Result<(), ScalewayBackendError> {
        if root_volume_id.trim().is_empty() {
            return Err(ScalewayBackendError::VolumeNotFound {
                volume_id: String::from("0"),
                zone: handle.zone.clone(),
            });
        }

        let mut volumes = HashMap::new();

        // Preserve root volume at index "0"
        volumes.insert(
            String::from("0"),
            VolumeAttachment {
                id: root_volume_id,
                boot: true,
            },
        );

        // Add cache volume at index "1"
        volumes.insert(
            String::from("1"),
            VolumeAttachment {
                id: volume_id.to_owned(),
                boot: false,
            },
        );

        let request = UpdateInstanceVolumesRequest { volumes };
        self.patch_instance_volumes(handle, &request, VolumePatchContext::attach(volume_id))
            .await
    }

    /// Sends a PATCH request to update instance volumes.
    pub(super) async fn patch_instance_volumes(
        &self,
        handle: &InstanceHandle,
        request: &UpdateInstanceVolumesRequest,
        context: VolumePatchContext<'_>,
    ) -> Result<(), ScalewayBackendError> {
        let url = format!(
            "{}/zones/{}/servers/{}",
            self.instance_api_base, handle.zone, handle.id
        );

        let response = super::HTTP_CLIENT
            .patch(&url)
            .header("X-Auth-Token", &self.config.secret_key)
            .json(request)
            .timeout(super::HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|err| ScalewayBackendError::Provider {
                message: err.to_string(),
            })?;

        if response.status().is_success() {
            return Ok(());
        }

        let error_text = response.text().await.unwrap_or_default();
        Err(context
            .action
            .into_error(context.volume_id.to_owned(), handle.id.clone(), error_text))
    }
}
