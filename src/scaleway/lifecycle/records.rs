//! Recording provider allocations, classifying unrecorded attachments, and
//! resolving the API family of a configured cache volume.
//!
//! Creation records everything the provider created with the server as owned.
//! Cleanup refreshes the server first and marks anything the ledger has never
//! seen as borrowed, so an attachment mriya did not create is detached and
//! preserved rather than deleted.

use super::models::{ServerModel, VolumeModel};
use crate::{
    ownership::{Disposition, ResourceKind, ResourceLedger, ResourceRef, StorageFamily},
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

impl ScalewayBackend {
    /// Records a configured cache volume as retained before it is attached.
    ///
    /// A volume the ledger already knows keeps its disposition: `mriya init`
    /// records its new volume as owned so a failure before formatting still
    /// deletes it. A volume the caller only borrowed from configuration is
    /// looked up so the recorded reference names the API family that must
    /// later detach and preserve it.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeNotFound`] when neither API knows
    /// the identifier, and the provider failure that prevented the lookup.
    pub(in crate::scaleway) async fn record_configured_cache(
        &self,
        volume_id: &str,
        zone: &str,
        ledger: &ResourceLedger,
    ) -> Result<StorageFamily, ScalewayBackendError> {
        if let Some(entry) = ledger
            .find(ResourceKind::InstanceVolume, volume_id)
            .or_else(|| ledger.find(ResourceKind::BlockVolume, volume_id))
        {
            return Ok(match entry.resource.kind() {
                ResourceKind::BlockVolume => StorageFamily::Block,
                _ => StorageFamily::Instance,
            });
        }

        match self.get_instance_volume(zone, volume_id).await {
            Ok(volume) => {
                // Derive the reference and the returned family from the same
                // value: an `sbs*` response must not be recorded as an
                // instance volume while reporting the Block family.
                let family = volume.storage_family();
                ledger.record(volume_ref(&volume, zone), Disposition::Retained);
                Ok(family)
            }
            Err(ScalewayBackendError::NotFound { .. }) => {
                match self.get_block_volume(zone, volume_id).await {
                    Ok(_) => {
                        ledger.record(
                            ResourceRef::block_volume(volume_id, zone),
                            Disposition::Retained,
                        );
                        Ok(StorageFamily::Block)
                    }
                    Err(ScalewayBackendError::NotFound { .. }) => {
                        Err(ScalewayBackendError::VolumeNotFound {
                            volume_id: volume_id.to_owned(),
                            zone: zone.to_owned(),
                        })
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }
}

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

/// Records resources an owned server reports that the ledger never saw.
///
/// This runs at cleanup, after the server has been refreshed. A resource
/// already recorded keeps its disposition: an owned allocation is never
/// downgraded, and a retained cache volume stays retained.
///
/// An unrecorded *dynamic* public IP on a run-owned server is recorded as
/// `Owned`: mriya only ever creates servers with a routed dynamic address, so
/// such an address was allocated for this run, and it can appear after the
/// single post-power-on refresh. Recording it as borrowed would leak it.
/// An unrecorded *static* IP is recorded as `Borrowed`: a pre-existing address
/// attached after creation is the user's, and requesting a dynamic address at
/// creation does not establish ownership of every address attached later.
/// An unrecorded *volume* stays `Borrowed`: it may be a user's, attached after
/// mriya read the server.
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
            let disposition = if ip.dynamic {
                Disposition::Owned
            } else {
                Disposition::Borrowed
            };
            ledger.record(
                ResourceRef::public_ip(&ip.id, &zone, ip.dynamic),
                disposition,
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
