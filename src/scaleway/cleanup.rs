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
        cleanup_support::{CleanupAccumulator, DetachOutcome, DetachSet, VolumeState, owned_ids},
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
        self.reconcile_missing_volume(ledger, &mut acc).await;

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

    /// Records a volume found by tag when an interrupted create recorded none.
    ///
    /// Without this lookup the outcome could be resolved and the journal
    /// removed while a volume the provider accepted stayed allocated. A lookup
    /// failure leaves the unknown-outcome flag set, so the journal survives
    /// until a later recovery can reconcile it.
    async fn reconcile_missing_volume(
        &self,
        ledger: &ResourceLedger,
        acc: &mut CleanupAccumulator,
    ) {
        // A create that succeeded can still have failed to write its volume
        // to the journal; the tag is then the only durable way back to it, so
        // the lookup runs whenever no volume was recorded.
        if !ledger.entries_of(ResourceKind::InstanceVolume).is_empty() {
            return;
        }
        let scope = ledger.scope();
        match self.reconcile_volume(&ledger.tag(), &scope).await {
            Ok(Some(volume)) => {
                let zone = if volume.zone.trim().is_empty() {
                    scope.zone.clone()
                } else {
                    volume.zone.clone()
                };
                ledger.record(
                    ResourceRef::instance_volume(&volume.id, &zone),
                    crate::ownership::Disposition::Owned,
                );
            }
            Ok(None) => {}
            Err(error) => acc.add_cause(format!("volume reconciliation failed: {error}")),
        }
    }

    /// Cleans one owned server, if the provider still has it.
    async fn clean_owned_server(
        &self,
        server_ref: &ResourceRef,
        ledger: &ResourceLedger,
        acc: &mut CleanupAccumulator,
    ) {
        // Every operation below is scoped to the ledger's zone, so a record
        // from another zone would be read and deleted through the wrong one.
        if server_ref.zone() != ledger.scope().zone {
            acc.add(
                server_ref.clone(),
                format!(
                    "recorded in zone {zone}, but the ledger is scoped to {scope}",
                    zone = server_ref.zone(),
                    scope = ledger.scope().zone
                ),
            );
            return;
        }
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

        if let Err(error) = self
            .power_off_if_needed(&zone, &server.snapshot(), &acc.project_id)
            .await
        {
            acc.add(server_ref, format!("power off failed: {error}"));
            return;
        }

        // The detach set is derived from a fresh read taken after power-off:
        // a volume attached between the first read and now would otherwise be
        // missing from the set and still attached when the server is deleted.
        let refreshed = match self.get_server(&zone, &server.id).await {
            Ok(refreshed) => refreshed,
            Err(ScalewayBackendError::NotFound { .. }) => {
                // The server disappeared on its own; the caller's absence check
                // records that, and there is nothing left to detach.
                self.delete_and_verify_server(&server_ref, acc).await;
                return;
            }
            Err(error) => {
                acc.add(server_ref, format!("refresh before delete failed: {error}"));
                return;
            }
        };
        let detach = DetachSet::for_server(&refreshed, ledger, &zone);
        let still_attached = match self.detach_preserved(&handle, &detach, acc).await {
            DetachOutcome::Detached => false,
            DetachOutcome::ServerGone => {
                self.delete_and_verify_server(&server_ref, acc).await;
                return;
            }
            DetachOutcome::Failed => true,
        };
        if still_attached {
            acc.add(
                server_ref,
                "not deleted while a preserved volume may remain attached",
            );
            return;
        }

        self.delete_and_verify_server(&server_ref, acc).await;
    }

    /// Detaches `detach` and reports any failure.
    async fn detach_preserved(
        &self,
        handle: &InstanceHandle,
        detach: &DetachSet<'_>,
        acc: &mut CleanupAccumulator,
    ) -> DetachOutcome {
        match self.detach_volumes(handle, &detach.volume_ids).await {
            Ok(()) => DetachOutcome::Detached,
            // The server vanished between the refresh and the patch, so no
            // preserved volume remains attached to it; the caller verifies the
            // absence and records it as removed.
            Err(ScalewayBackendError::NotFound { .. }) => DetachOutcome::ServerGone,
            Err(error) => {
                for volume in detach.attached_volumes() {
                    acc.add(
                        volume_ref(volume, detach.zone),
                        format!("detach failed: {error}"),
                    );
                }
                DetachOutcome::Failed
            }
        }
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
    pub(super) async fn volume_state(
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
