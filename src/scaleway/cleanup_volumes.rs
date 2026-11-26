//! Volume deletion for ownership-aware cleanup.
//!
//! Volumes are removed after their server, so a local volume is no longer
//! attached. A Block Storage volume can still report `in_use` for a moment
//! after the server goes, so it is given a bounded wait before deletion, and
//! a volume that stays in use is reported as a residual rather than retried
//! forever.

use crate::{
    ownership::{ResourceKind, ResourceLedger, ResourceRef},
    scaleway::{
        ScalewayBackend,
        ScalewayBackendError,
        cleanup_support::{CleanupAccumulator, VolumeState, owned_ids},
    },
};

impl ScalewayBackend {
    /// Deletes every owned volume, waiting for block volumes to leave use.
    pub(super) async fn clean_owned_volumes(
        &self,
        ledger: &ResourceLedger,
        acc: &mut CleanupAccumulator,
    ) {
        let volumes = owned_ids(ledger, ResourceKind::InstanceVolume)
            .into_iter()
            .chain(owned_ids(ledger, ResourceKind::BlockVolume));
        for resource in volumes {
            self.delete_owned_volume(&resource, acc).await;
        }
    }

    /// Deletes one owned volume and verifies its absence.
    async fn delete_owned_volume(&self, resource: &ResourceRef, acc: &mut CleanupAccumulator) {
        match self.volume_state(resource, &acc.project_id).await {
            Ok(VolumeState::Absent) => acc.removed(resource.clone()),
            Ok(VolumeState::AttachedTo(server)) => {
                self.clean_attached_volume(resource, &server, acc).await;
            }
            Ok(VolumeState::Detached) => {
                if resource.kind() == ResourceKind::BlockVolume
                    && let Err(error) = self
                        .wait_for_block_volume_idle(resource.zone(), resource.id())
                        .await
                {
                    acc.add(resource.clone(), error.to_string());
                    return;
                }
                self.delete_and_verify_volume(resource, acc).await;
            }
            Err(error) => acc.add(resource.clone(), error.to_string()),
        }
    }

    /// Handles an owned volume the provider still reports as attached.
    ///
    /// A block volume released with its server can take a moment to leave
    /// `in_use`, so it is given a short wait before being reported as a
    /// residual; a local volume keeps its attached state instead, because
    /// deleting it while attached is rejected.
    async fn clean_attached_volume(
        &self,
        resource: &ResourceRef,
        server: &str,
        acc: &mut CleanupAccumulator,
    ) {
        if resource.kind() != ResourceKind::BlockVolume {
            acc.add(
                resource.clone(),
                format!("still attached to server {server}"),
            );
            return;
        }
        match self
            .wait_for_block_volume_idle(resource.zone(), resource.id())
            .await
        {
            Ok(()) => self.delete_and_verify_volume(resource, acc).await,
            Err(error) => acc.add(resource.clone(), format!("{server}: {error}")),
        }
    }

    /// Deletes a detached owned volume and confirms it is gone.
    async fn delete_and_verify_volume(&self, resource: &ResourceRef, acc: &mut CleanupAccumulator) {
        let deleted = match resource.kind() {
            ResourceKind::BlockVolume => {
                self.delete_block_volume(resource.zone(), resource.id())
                    .await
            }
            _ => {
                self.delete_instance_volume(resource.zone(), resource.id())
                    .await
            }
        };

        if let Err(error) = deleted
            && !matches!(error, ScalewayBackendError::NotFound { .. })
        {
            acc.add(resource.clone(), format!("delete failed: {error}"));
            return;
        }

        match self.volume_state(resource, &acc.project_id).await {
            Ok(VolumeState::Absent) => acc.removed(resource.clone()),
            Ok(_) => acc.add(resource.clone(), "volume is still present after deletion"),
            Err(error) => acc.add(resource.clone(), error.to_string()),
        }
    }
}
