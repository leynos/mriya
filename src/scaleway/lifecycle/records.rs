//! Recording provider allocations and classifying unrecorded attachments.
//!
//! Creation records everything the provider created with the server as owned.
//! Cleanup refreshes the server first and marks anything the ledger has never
//! seen as borrowed, so an attachment mriya did not create is detached and
//! preserved rather than deleted.

use super::models::{ServerModel, VolumeModel};
use crate::ownership::{Disposition, ResourceKind, ResourceLedger, ResourceRef, StorageFamily};

/// Builds the reference recorded for `volume` in `zone`.
///
/// The zone comes from the ledger scope rather than the volume object, because
/// some provider responses omit it and a reference without a zone cannot be
/// checked or deleted.
pub(in crate::scaleway) fn volume_ref(volume: &VolumeModel, zone: &str) -> ResourceRef {
    match volume.storage_family() {
        StorageFamily::Block => ResourceRef::block_volume(&volume.id, zone),
        StorageFamily::Instance => ResourceRef::instance_volume(&volume.id, zone),
    }
}

/// Records everything a server was created with as owned.
///
/// The server, its root volume, and every public IP allocated with it are
/// billable resources of this run. The image is recorded separately as
/// borrowed.
pub(in crate::scaleway) fn record_server_creation(server: &ServerModel, ledger: &ResourceLedger) {
    let zone = ledger.scope().zone;
    ledger.record(ResourceRef::server(&server.id, &zone), Disposition::Owned);
    if let Some(root) = server.root_volume() {
        ledger.record(volume_ref(root, &zone), Disposition::Owned);
    }
    for ip in server.ip_allocations() {
        ledger.record(
            ResourceRef::public_ip(&ip.id, &zone, ip.dynamic),
            Disposition::Owned,
        );
    }
}

/// Marks attachments and IPs the ledger has never seen as borrowed.
///
/// This runs at cleanup, after the server has been refreshed. A resource
/// already recorded keeps its disposition: an owned allocation is never
/// downgraded, and a retained cache volume stays retained.
pub(in crate::scaleway) fn classify_unrecorded(server: &ServerModel, ledger: &ResourceLedger) {
    let zone = ledger.scope().zone;
    for volume in server.volumes.values() {
        let resource = volume_ref(volume, &zone);
        if ledger.find(resource.kind(), resource.id()).is_none() {
            ledger.record(resource, Disposition::Borrowed);
        }
    }
    for ip in server.ip_allocations() {
        if ledger.find(ResourceKind::PublicIp, &ip.id).is_none() {
            ledger.record(
                ResourceRef::public_ip(&ip.id, &zone, ip.dynamic),
                Disposition::Borrowed,
            );
        }
    }
}

/// Returns the disposition recorded for an attached volume.
pub(in crate::scaleway) fn disposition_of(
    ledger: &ResourceLedger,
    volume: &VolumeModel,
    zone: &str,
) -> Option<Disposition> {
    let resource = volume_ref(volume, zone);
    ledger
        .find(resource.kind(), resource.id())
        .map(|entry| entry.disposition)
}
