//! Orderly teardown that protects an attached cache volume.
//!
//! Scaleway deletes every volume attached to a server when the server is
//! deleted through the API. Teardown therefore stops the server, detaches the
//! cache, waits until the volume is `available`, and only then deletes the
//! server. If the detach fails, the server is retained: a visible paid server
//! is safer than silently deleting the cache.

use super::super::{ScalewayBackend, ScalewayBackendError};
use crate::{
    backend::{Backend, InstanceHandle},
    volume::{TeardownFailure, TeardownStage},
};

impl ScalewayBackend {
    /// Stops the instance when needed, detaches the cache, and destroys it.
    ///
    /// # Errors
    ///
    /// Returns a [`TeardownFailure`] tagged with the stage that failed. A
    /// detach failure carries [`ScalewayBackendError::VolumeDetachRetainsServer`]
    /// and leaves the server intact; a destroy failure means the cache was
    /// already detached. A server is never destroyed while the cache is still
    /// attached.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // With a cache, the volume is available before the server disappears.
    /// backend.teardown_instance(&handle, Some("vol-cache")).await?;
    ///
    /// // Without a cache the call is a plain destroy.
    /// backend.teardown_instance(&handle, None).await?;
    /// ```
    pub(in crate::scaleway) async fn teardown_instance(
        &self,
        handle: &InstanceHandle,
        volume_id: Option<&str>,
    ) -> Result<(), TeardownFailure<ScalewayBackendError>> {
        if let Some(id) = volume_id {
            self.power_off_if_needed(handle)
                .await
                .map_err(|source| TeardownFailure {
                    stage: TeardownStage::Stopping,
                    source,
                })?;
            if let Err(error) = self.detach_volume(handle, id).await {
                let source = ScalewayBackendError::VolumeDetachRetainsServer {
                    volume_id: id.to_owned(),
                    instance_id: handle.id.clone(),
                    zone: handle.zone.clone(),
                    message: error.to_string(),
                };
                return Err(TeardownFailure {
                    stage: TeardownStage::Detaching,
                    source,
                });
            }
        }

        Backend::destroy(self, handle.clone())
            .await
            .map_err(|source| TeardownFailure {
                stage: TeardownStage::Destroying,
                source,
            })
    }
}
