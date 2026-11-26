//! Ownership-aware cleanup for the Scaleway backend.
//!
//! Cleanup is driven entirely by the ledger. For each owned server it powers
//! the guest off, detaches every volume the ledger does not own as a transient
//! resource, deletes the server, and verifies it is gone. Owned volumes and
//! public IPs are then deleted through the API that owns each one and verified
//! absent. Retained and borrowed volumes are re-checked at the end: they must
//! still exist and must no longer be attached.
//!
//! Every step's result is collected, so one failure does not hide another; a
//! confirmed absence is success, and an authentication or authorization
//! failure is a failure, never a silent pass.

use crate::{
    backend::InstanceHandle,
    cleanup::{CleanupFailure, CleanupReport},
    ownership::{ResourceKind, ResourceLedger, ResourceRef},
    scaleway::{
        ScalewayBackend,
        ScalewayBackendError,
        cleanup_support::{CleanupAccumulator, DetachSet, VolumeState, owned_ids},
        lifecycle::{
            models::{Presence, ServerModel},
            records::{classify_unrecorded, record_server_creation, volume_ref},
        },
    },
};

impl ScalewayBackend {
    /// Removes every owned resource and verifies the resulting state.
    ///
    /// See [`crate::backend::Backend::cleanup`] for the contract.
    ///
    /// # Errors
    ///
    /// Returns [`CleanupFailure`] naming the residual resources and the causes
    /// that prevented verification.
    pub(crate) async fn cleanup_ledger(
        &self,
        ledger: &ResourceLedger,
    ) -> Result<CleanupReport, CleanupFailure> {
        let mut acc = CleanupAccumulator::new(ledger.scope().project_id);

        self.reconcile_missing_server(ledger, &mut acc).await;

        for server in owned_ids(ledger, ResourceKind::Server) {
            self.clean_owned_server(&server, ledger, &mut acc).await;
        }

        self.clean_owned_volumes(ledger, &mut acc).await;
        self.clean_owned_ips(ledger, &mut acc).await;
        self.verify_preserved(ledger, &mut acc).await;

        acc.report.warnings.extend(ledger.warnings());
        let CleanupAccumulator {
            report, failure, ..
        } = acc;
        if failure.is_empty() {
            return Ok(report);
        }
        Err(failure)
    }

    /// Records a server found by tag when the ledger has no server identifier.
    ///
    /// This is the recovery path for an interrupted allocation: the tag was
    /// persisted before the create request, and the server it found is ours.
    async fn reconcile_missing_server(
        &self,
        ledger: &ResourceLedger,
        acc: &mut CleanupAccumulator,
    ) {
        if !ledger.entries_of(ResourceKind::Server).is_empty() {
            return;
        }
        let scope = ledger.scope();
        match self.find_server_by_tag(&ledger.tag(), &scope).await {
            Ok(Some(server)) => record_server_creation(&server, ledger),
            Ok(None) => {}
            Err(error) => acc.add_cause(format!("server reconciliation failed: {error}")),
        }
    }

    /// Cleans one owned server, if the provider still has it.
    async fn clean_owned_server(
        &self,
        server_ref: &ResourceRef,
        ledger: &ResourceLedger,
        acc: &mut CleanupAccumulator,
    ) {
        match self.server_presence(server_ref, &acc.project_id).await {
            Ok(Presence::ConfirmedAbsent) => acc.removed(server_ref.clone()),
            Ok(Presence::Present(server)) => {
                classify_unrecorded(&server, ledger);
                self.clean_present_server(&server, ledger, acc).await;
            }
            Err(error) => acc.add(server_ref.clone(), error.to_string()),
        }
    }

    /// Powers off, detaches preserved volumes, and deletes a present server.
    async fn clean_present_server(
        &self,
        server: &ServerModel,
        ledger: &ResourceLedger,
        acc: &mut CleanupAccumulator,
    ) {
        let zone = ledger.scope().zone;
        let server_ref = ResourceRef::server(&server.id, &zone);
        let handle = InstanceHandle {
            id: server.id.clone(),
            zone: zone.clone(),
        };

        if let Err(error) = self.power_off_if_needed(&zone, &server.snapshot()).await {
            acc.add(server_ref, format!("power off failed: {error}"));
            return;
        }

        let detach = DetachSet::for_server(server, ledger, &zone);
        if !self.detach_preserved(&handle, &detach, acc).await {
            acc.add(
                server_ref,
                "not deleted while a preserved volume may remain attached",
            );
            return;
        }

        self.delete_and_verify_server(&server_ref, acc).await;
    }

    /// Detaches `detach` and reports any failure.
    ///
    /// Returns `false` when a preserved volume may remain attached, which is
    /// the condition that forbids deleting the server.
    async fn detach_preserved(
        &self,
        handle: &InstanceHandle,
        detach: &DetachSet<'_>,
        acc: &mut CleanupAccumulator,
    ) -> bool {
        let Err(error) = self.detach_volumes(handle, &detach.volume_ids).await else {
            return true;
        };
        for volume in detach.attached_volumes() {
            acc.add(
                volume_ref(volume, detach.zone),
                format!("detach failed: {error}"),
            );
        }
        false
    }

    /// Deletes the server and confirms its absence.
    async fn delete_and_verify_server(
        &self,
        server_ref: &ResourceRef,
        acc: &mut CleanupAccumulator,
    ) {
        match self.delete_server(server_ref.zone(), server_ref.id()).await {
            Ok(()) | Err(ScalewayBackendError::NotFound { .. }) => {}
            Err(error) => {
                acc.add(server_ref.clone(), format!("delete failed: {error}"));
                return;
            }
        }

        match self.server_presence(server_ref, &acc.project_id).await {
            Ok(Presence::ConfirmedAbsent) => acc.removed(server_ref.clone()),
            Ok(Presence::Present(_)) => {
                acc.add(server_ref.clone(), "server is still present after deletion");
            }
            Err(error) => acc.add(server_ref.clone(), error.to_string()),
        }
    }

    /// Deletes every owned volume, waiting for block volumes to leave use.
    async fn clean_owned_volumes(&self, ledger: &ResourceLedger, acc: &mut CleanupAccumulator) {
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
                acc.add(
                    resource.clone(),
                    format!("still attached to server {server}"),
                );
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

    /// Releases every owned public IP and verifies its absence.
    async fn clean_owned_ips(&self, ledger: &ResourceLedger, acc: &mut CleanupAccumulator) {
        for resource in owned_ids(ledger, ResourceKind::PublicIp) {
            self.clean_owned_ip(&resource, acc).await;
        }
    }

    /// Releases one owned public IP and verifies its absence.
    async fn clean_owned_ip(&self, resource: &ResourceRef, acc: &mut CleanupAccumulator) {
        match self.ip_presence(resource, &acc.project_id).await {
            Ok(Presence::ConfirmedAbsent) => acc.removed(resource.clone()),
            Ok(Presence::Present(_)) => {
                self.release_present_ip(resource, acc).await;
            }
            Err(error) => acc.add(resource.clone(), error.to_string()),
        }
    }

    /// Releases an IP the provider still reports, then re-checks its absence.
    async fn release_present_ip(&self, resource: &ResourceRef, acc: &mut CleanupAccumulator) {
        if let Err(error) = self.delete_ip(resource.zone(), resource.id()).await
            && !matches!(error, ScalewayBackendError::NotFound { .. })
        {
            acc.add(resource.clone(), format!("release failed: {error}"));
            return;
        }
        match self.ip_presence(resource, &acc.project_id).await {
            Ok(Presence::ConfirmedAbsent) => acc.removed(resource.clone()),
            Ok(Presence::Present(_)) => {
                acc.add(resource.clone(), "ip is still present after release");
            }
            Err(error) => acc.add(resource.clone(), error.to_string()),
        }
    }

    /// Confirms every retained and borrowed volume survived, detached.
    async fn verify_preserved(&self, ledger: &ResourceLedger, acc: &mut CleanupAccumulator) {
        let preserved = ledger
            .entries()
            .into_iter()
            .filter(|entry| !entry.disposition.is_deletable())
            .filter(|entry| {
                matches!(
                    entry.resource.kind(),
                    ResourceKind::InstanceVolume | ResourceKind::BlockVolume
                )
            })
            .map(|entry| entry.resource)
            .collect::<Vec<_>>();

        for resource in preserved {
            match self.volume_state(&resource, &acc.project_id).await {
                Ok(VolumeState::Detached) => acc.preserved(resource),
                Ok(VolumeState::AttachedTo(server)) => acc.add(
                    resource,
                    format!("preserved volume is still attached to server {server}"),
                ),
                Ok(VolumeState::Absent) => {
                    acc.add(resource, "preserved volume is missing");
                }
                Err(error) => acc.add(resource, error.to_string()),
            }
        }
    }

    /// Resolves the state of a recorded volume through its owning API.
    async fn volume_state(
        &self,
        resource: &ResourceRef,
        project_id: &str,
    ) -> Result<VolumeState, ScalewayBackendError> {
        if resource.kind() == ResourceKind::BlockVolume {
            return match self.block_volume_presence(resource, project_id).await? {
                Presence::ConfirmedAbsent => Ok(VolumeState::Absent),
                Presence::Present(volume) if volume.is_in_use() => {
                    Ok(VolumeState::AttachedTo(String::from("an instance")))
                }
                Presence::Present(_) => Ok(VolumeState::Detached),
            };
        }

        match self.instance_volume_presence(resource, project_id).await? {
            Presence::ConfirmedAbsent => Ok(VolumeState::Absent),
            Presence::Present(volume) => {
                match volume.server.filter(|server| !server.id.trim().is_empty()) {
                    Some(server) => Ok(VolumeState::AttachedTo(server.id)),
                    None => Ok(VolumeState::Detached),
                }
            }
        }
    }
}
