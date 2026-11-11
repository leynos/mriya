//! Block Storage volume detachment helpers for the Scaleway backend.

use std::collections::HashMap;

use crate::backend::InstanceHandle;

use super::super::volume::{UpdateInstanceVolumesRequest, VolumeAttachment};
use super::super::{ScalewayBackend, ScalewayBackendError};
use super::volume_attach::VolumePatchContext;

impl ScalewayBackend {
    /// Detaches a volume from the instance while preserving the root volume.
    ///
    /// The instance is re-read first so the replacement payload keeps whatever
    /// the provider currently reports at index `"0"`. Reading through the
    /// backend's own HTTP boundary, rather than the SDK helper, lets callers
    /// substitute a loopback API.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeNotFound`] when the instance
    /// reports no root volume, and [`ScalewayBackendError::VolumeDetachFailed`]
    /// when the API rejects the detachment request.
    pub(in crate::scaleway) async fn detach_volume(
        &self,
        handle: &InstanceHandle,
        volume_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        let instance = self.get_instance(handle).await?;

        let root_volume_id = instance
            .volumes
            .volumes
            .get("0")
            .map(|volume| volume.id.clone())
            .ok_or_else(|| ScalewayBackendError::VolumeNotFound {
                volume_id: String::from("0"),
                zone: handle.zone.clone(),
            })?;

        let mut volumes = HashMap::new();
        volumes.insert(
            String::from("0"),
            VolumeAttachment {
                id: root_volume_id,
                boot: true,
            },
        );

        let request = UpdateInstanceVolumesRequest { volumes };
        self.patch_instance_volumes(handle, &request, VolumePatchContext::detach(volume_id))
            .await
    }
}
