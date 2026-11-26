//! Provider inventory for one zone and project.
//!
//! Live verification compares the inventory before and after a scenario, and
//! the comparison is what proves a run left no server, volume, or address
//! behind. The inventory is deliberately unfiltered: filtering by tag would
//! hide exactly the untagged residue the check exists to catch.

use crate::{
    ownership::{AllocationScope, ResourceRef},
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

/// Every server, volume, and public IP held in one zone and project.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceInventory {
    /// Compute servers.
    pub servers: Vec<ResourceRef>,
    /// Volumes managed by the Instances API.
    pub instance_volumes: Vec<ResourceRef>,
    /// Volumes managed by the Block Storage API.
    pub block_volumes: Vec<ResourceRef>,
    /// Public IP allocations.
    pub public_ips: Vec<ResourceRef>,
}

impl ResourceInventory {
    /// Reports whether the scope holds no resources at all.
    ///
    /// # Returns
    ///
    /// `true` when no server, volume, or IP was listed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.servers.is_empty()
            && self.instance_volumes.is_empty()
            && self.block_volumes.is_empty()
            && self.public_ips.is_empty()
    }

    /// Returns the total number of resources.
    ///
    /// # Returns
    ///
    /// The sum of the four per-kind counts.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.servers.len()
            + self.instance_volumes.len()
            + self.block_volumes.len()
            + self.public_ips.len()
    }

    /// Returns every resource reference, sorted so two inventories compare
    /// equal when they hold the same resources.
    ///
    /// # Returns
    ///
    /// All references from the four lists, ordered by kind and then identifier.
    #[must_use]
    pub fn all(&self) -> Vec<ResourceRef> {
        let mut resources = Vec::with_capacity(self.len());
        resources.extend(self.servers.iter().cloned());
        resources.extend(self.instance_volumes.iter().cloned());
        resources.extend(self.block_volumes.iter().cloned());
        resources.extend(self.public_ips.iter().cloned());
        resources.sort_by(|left, right| {
            left.kind()
                .as_str()
                .cmp(right.kind().as_str())
                .then_with(|| left.id().cmp(right.id()))
        });
        resources
    }
}

impl ScalewayBackend {
    /// Lists every server, volume, and public IP in `scope`.
    ///
    /// # Parameters
    ///
    /// - `scope`: the zone and project to inventory.
    ///
    /// # Returns
    ///
    /// A [`ResourceInventory`] holding one reference per listed resource.
    ///
    /// The four listings are independent reads, so a resource created or
    /// deleted between them can make the result inconsistent with any single
    /// instant. It is a verification aid for a quiescent project, not a
    /// transactional snapshot.
    ///
    /// # Errors
    ///
    /// Returns the first listing failure.
    pub async fn inventory(
        &self,
        scope: &AllocationScope,
    ) -> Result<ResourceInventory, ScalewayBackendError> {
        let servers = self
            .list_servers(&scope.zone, &scope.project_id, None)
            .await?;
        let instance_volumes = self
            .list_instance_volumes(&scope.zone, &scope.project_id, None)
            .await?;
        let block_volumes = self
            .list_block_volumes(&scope.zone, &scope.project_id, None)
            .await?;
        let ips = self.list_ips(&scope.zone, &scope.project_id).await?;

        let zone = scope.zone.as_str();
        Ok(ResourceInventory {
            servers: servers
                .iter()
                .map(|server| ResourceRef::server(&server.id, zone))
                .collect(),
            instance_volumes: instance_volumes
                .iter()
                .map(|volume| ResourceRef::instance_volume(&volume.id, zone))
                .collect(),
            block_volumes: block_volumes
                .iter()
                .map(|volume| ResourceRef::block_volume(&volume.id, zone))
                .collect(),
            public_ips: ips
                .iter()
                .map(|ip| ResourceRef::public_ip(&ip.id, zone, ip.dynamic))
                .collect(),
        })
    }
}
