//! Local response models for the provider payloads mriya reads.
//!
//! The SDK's `ScalewayInstance` requires about thirty fields with no defaults,
//! so it cannot decode a partial response and cannot express the identity
//! checks cleanup needs. These models keep only what the lifecycle reads, give
//! every optional field a default, and are the single place that knows how a
//! provider volume family maps to the API that owns it.

use std::collections::HashMap;

use serde::Deserialize;

use super::InstanceSnapshot;
use crate::{
    ownership::StorageFamily,
    scaleway::types::{Action, InstanceId, InstanceState},
};

/// Response wrapper for a single server.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct ServerResponse {
    /// The server object.
    pub(in crate::scaleway) server: ServerModel,
}

/// Response wrapper for a server list page.
///
/// `servers` is required: a 200 body without it is not a listing, and
/// defaulting it to empty would let the presence check report a server as
/// confirmed absent from a response that never listed anything.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct ServerList {
    /// Servers on this page.
    pub(in crate::scaleway) servers: Vec<ServerModel>,
}

/// The subset of a server object the lifecycle reads.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct ServerModel {
    /// Provider identifier.
    pub(in crate::scaleway) id: String,
    /// Zone that holds the server.
    #[serde(default)]
    pub(in crate::scaleway) zone: String,
    /// Project that owns the server.
    #[serde(default)]
    pub(in crate::scaleway) project: String,
    /// Provider-reported state, such as `running` or `stopped`.
    #[serde(default)]
    pub(in crate::scaleway) state: String,
    /// Tags attached to the server.
    #[serde(default)]
    pub(in crate::scaleway) tags: Vec<String>,
    /// Actions the provider currently permits.
    #[serde(default)]
    pub(in crate::scaleway) allowed_actions: Vec<String>,
    /// Legacy single public IP, still present in some responses.
    #[serde(default)]
    pub(in crate::scaleway) public_ip: Option<PublicIpModel>,
    /// Public IPs attached to the server.
    #[serde(default)]
    pub(in crate::scaleway) public_ips: Vec<PublicIpModel>,
    /// Volumes attached to the server, keyed by index (`"0"` is the root).
    #[serde(default)]
    pub(in crate::scaleway) volumes: HashMap<String, VolumeModel>,
}

impl ServerModel {
    /// Reduces the server to the snapshot lifecycle polling reads.
    #[must_use]
    pub(in crate::scaleway) fn snapshot(&self) -> InstanceSnapshot {
        InstanceSnapshot {
            id: InstanceId::from(self.id.clone()),
            state: InstanceState::from(self.state.clone()),
            allowed_actions: self
                .allowed_actions
                .iter()
                .cloned()
                .map(Action::from)
                .collect(),
            public_ip: self.public_ip_address(),
        }
    }

    /// Returns the first parseable public IPv4 address, if any.
    ///
    /// A response may carry IPv6 entries alongside IPv4 ones, and the field
    /// this feeds is documented as IPv4, so each candidate is parsed rather
    /// than merely checked for emptiness. The legacy single-IP field is
    /// consulted after the list.
    #[must_use]
    pub(in crate::scaleway) fn public_ip_address(&self) -> Option<String> {
        self.public_ips
            .iter()
            .chain(self.public_ip.iter())
            .map(|ip| ip.address.trim())
            .find(|address| address.parse::<std::net::Ipv4Addr>().is_ok())
            .map(str::to_owned)
    }

    /// Returns every distinct public IP attached to the server.
    #[must_use]
    pub(in crate::scaleway) fn ip_allocations(&self) -> Vec<&PublicIpModel> {
        let mut seen: Vec<&str> = Vec::new();
        let mut allocations = Vec::new();
        for ip in self.public_ips.iter().chain(self.public_ip.iter()) {
            if ip.id.trim().is_empty() || seen.contains(&ip.id.as_str()) {
                continue;
            }
            seen.push(ip.id.as_str());
            allocations.push(ip);
        }
        allocations
    }

    /// Returns the volume mapped at index `"0"`.
    #[must_use]
    pub(in crate::scaleway) fn root_volume(&self) -> Option<&VolumeModel> { self.volumes.get("0") }

    /// Reports whether the server carries `tag` exactly.
    #[must_use]
    pub(in crate::scaleway) fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|existing| existing == tag)
    }

    /// Reports whether the server lives in `zone`, accepting an omitted zone.
    #[must_use]
    pub(in crate::scaleway) fn is_in_zone(&self, zone: &str) -> bool {
        self.zone.trim().is_empty() || self.zone == zone
    }

    /// Reports whether the server belongs to `project`, accepting an omitted
    /// project.
    #[must_use]
    pub(in crate::scaleway) fn is_in_project(&self, project: &str) -> bool {
        self.project.trim().is_empty() || self.project == project
    }
}

/// A public IP attached to a server or listed on its own.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct PublicIpModel {
    /// Provider identifier.
    pub(in crate::scaleway) id: String,
    /// Dotted IPv4 address.
    #[serde(default)]
    pub(in crate::scaleway) address: String,
    /// Whether the provider treats this as a dynamic address.
    #[serde(default)]
    pub(in crate::scaleway) dynamic: bool,
}

/// Response wrapper for a single IP.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct IpResponse {
    /// The IP object.
    pub(in crate::scaleway) ip: PublicIpModel,
}

/// Response wrapper for an IP list page.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct IpList {
    /// IPs on this page.
    #[serde(default)]
    pub(in crate::scaleway) ips: Vec<PublicIpModel>,
}

/// The server reference embedded in a volume object.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct VolumeServerModel {
    /// Identifier of the server the volume is attached to.
    #[serde(default)]
    pub(in crate::scaleway) id: String,
}

/// The subset of an Instances API volume the lifecycle reads.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct VolumeModel {
    /// Provider identifier.
    pub(in crate::scaleway) id: String,
    /// Provider volume type, such as `l_ssd` or `b_ssd`.
    #[serde(default, rename = "volume_type")]
    pub(in crate::scaleway) volume_type: String,
    /// Zone that holds the volume.
    #[serde(default)]
    pub(in crate::scaleway) zone: String,
    /// Tags attached to the volume.
    #[serde(default)]
    pub(in crate::scaleway) tags: Vec<String>,
    /// Server the volume is attached to, when attached.
    #[serde(default)]
    pub(in crate::scaleway) server: Option<VolumeServerModel>,
}

impl VolumeModel {
    /// Returns the API family that owns this volume.
    #[must_use]
    pub(in crate::scaleway) fn storage_family(&self) -> StorageFamily {
        if self.volume_type.starts_with("sbs") {
            StorageFamily::Block
        } else {
            StorageFamily::Instance
        }
    }
}

/// Response wrapper for a single Instances API volume.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct InstanceVolumeResponse {
    /// The volume object.
    pub(in crate::scaleway) volume: VolumeModel,
}

/// Response wrapper for an Instances API volume list page.
///
/// `volumes` is required for the same reason as `ServerList::servers`.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct InstanceVolumeList {
    /// Volumes on this page.
    pub(in crate::scaleway) volumes: Vec<VolumeModel>,
}

/// A Block Storage API volume, which uses a different identifier namespace and
/// status vocabulary from its Instances API counterpart.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct BlockVolumeModel {
    /// Provider identifier.
    pub(in crate::scaleway) id: String,
    /// Provider status, such as `available` or `in_use`.
    #[serde(default)]
    pub(in crate::scaleway) status: String,
    /// Zone that holds the volume.
    #[serde(default)]
    pub(in crate::scaleway) zone: String,
    /// Project that owns the volume.
    #[serde(default)]
    pub(in crate::scaleway) project_id: String,
    /// Tags attached to the volume.
    #[serde(default)]
    pub(in crate::scaleway) tags: Vec<String>,
}

impl BlockVolumeModel {
    /// Reports whether the volume is still attached to an instance.
    #[must_use]
    pub(in crate::scaleway) fn is_in_use(&self) -> bool { self.status == "in_use" }
}

/// Response wrapper for a Block Storage volume list page.
///
/// `volumes` is required for the same reason as `ServerList::servers`.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::scaleway) struct BlockVolumeList {
    /// Volumes on this page.
    pub(in crate::scaleway) volumes: Vec<BlockVolumeModel>,
}

/// Result of checking whether a recorded resource still exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::scaleway) enum Presence<T> {
    /// The provider confirmed the resource still exists.
    Present(T),
    /// A successful, identity-checked listing confirmed the resource is gone.
    ConfirmedAbsent,
}

impl<T> Presence<T> {
    /// Reports whether a successful listing confirmed the resource is gone.
    #[must_use]
    pub(in crate::scaleway) const fn is_absent(&self) -> bool {
        matches!(self, Self::ConfirmedAbsent)
    }
}
