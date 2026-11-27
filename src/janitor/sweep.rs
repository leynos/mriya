//! Boot-volume discovery and removal for the janitor sweep.
//!
//! The root volume of a tagged server carries no tag of its own, so its
//! identifier is recorded before the server is deleted. A failed server delete
//! must not discard that identifier: the sweep still attempts every recorded
//! root and reports each one that remains.

use super::{Janitor, JanitorError, ScwServer, boot_volume_zone};

impl<R: super::CommandRunner> Janitor<R> {
    /// Returns `(id, zone, is_block_storage)` for each server's root volume.
    ///
    /// An attached volume lives in the server's zone, so that zone is used when
    /// the volume object omits its own. The family is kept because an SBS root
    /// is invisible to the Instances API inventory and must be deleted through
    /// the Block Storage path.
    pub(super) fn boot_volume_refs(servers: &[ScwServer]) -> Vec<(String, String, bool)> {
        servers
            .iter()
            .filter_map(|server| {
                server.volumes.get("0").map(|volume| {
                    (
                        volume.id.clone(),
                        boot_volume_zone(volume, server),
                        volume.is_block_storage(),
                    )
                })
            })
            .collect()
    }

    /// Deletes any recorded boot volume the provider still has.
    ///
    /// The root volume is often removed with its server, so each identifier is
    /// checked against the project's Instance volumes first; only a volume that
    /// still exists is deleted. A failure here is reported, because the
    /// untagged volume has no other path back into a later sweep.
    pub(super) fn delete_boot_volumes(
        &self,
        boot_volumes: &[(String, String, bool)],
        first_error: &mut Option<JanitorError>,
    ) -> Result<(), JanitorError> {
        // The recorded zone is only used for the residual message; deletion
        // sends the zone the provider reports for the volume.
        if boot_volumes.is_empty() {
            return Ok(());
        }
        let instance = self.list_instance_volumes()?;
        let block = self.list_block_volumes()?;
        for (id, _zone, is_block) in boot_volumes {
            let candidate = if *is_block {
                block.iter().find(|volume| &volume.id == id)
            } else {
                instance.iter().find(|volume| &volume.id == id)
            };
            let Some(volume) = candidate else {
                continue;
            };
            // One failed root must not hide the others: the remaining roots are
            // still attempted and their identifiers reported below.
            let deleted = if *is_block {
                self.delete_volume(volume)
            } else {
                self.delete_instance_volume(volume)
            };
            if let Err(error) = deleted {
                first_error.get_or_insert(error);
            }
        }
        let remaining_instance = self.list_instance_volumes()?;
        let remaining_block = self.list_block_volumes()?;
        let leftovers = boot_volumes
            .iter()
            .filter(|(id, _, is_block)| {
                let inventory = if *is_block {
                    &remaining_block
                } else {
                    &remaining_instance
                };
                inventory.iter().any(|volume| &volume.id == id)
            })
            .map(|(id, zone, _)| format!("{id}@{zone}"))
            .collect::<Vec<_>>();
        if leftovers.is_empty() {
            return Ok(());
        }
        Err(JanitorError::NotClean {
            message: format!("boot volumes remaining: [{}]", leftovers.join(", ")),
        })
    }
}
