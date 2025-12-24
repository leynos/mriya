//! Block Storage volume attachment helpers for the Scaleway backend.

use std::collections::HashMap;

use crate::backend::InstanceHandle;

use super::super::volume::{UpdateInstanceVolumesRequest, VolumeAttachment};
use super::super::{ScalewayBackend, ScalewayBackendError};

#[derive(Copy, Clone, Debug)]
pub(super) enum VolumePatchAction {
    Attach,
    Detach,
}

impl VolumePatchAction {
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

#[derive(Copy, Clone, Debug)]
pub(super) struct VolumePatchContext<'a> {
    volume_id: &'a str,
    action: VolumePatchAction,
}

impl<'a> VolumePatchContext<'a> {
    pub(super) const fn attach(volume_id: &'a str) -> Self {
        Self {
            volume_id,
            action: VolumePatchAction::Attach,
        }
    }

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
            super::SCALEWAY_INSTANCE_API_BASE,
            handle.zone,
            handle.id
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
