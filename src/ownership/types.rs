//! Provider resource references, dispositions, and allocation scope types.
//!
//! These types are the vocabulary the ownership ledger and its recovery
//! journal share. They name *what* mriya allocated, *where* it lives, and
//! *how* mriya came to hold it, without depending on any provider client.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Prefix applied to the tag that correlates every resource of one run.
pub const ALLOCATION_TAG_PREFIX: &str = "mriya-alloc-";

/// Kind of provider resource that mriya may allocate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    /// A compute server (instance).
    Server,
    /// A volume managed through the Instances API.
    InstanceVolume,
    /// A volume managed through the Block Storage API.
    BlockVolume,
    /// A public IP address allocation.
    PublicIp,
    /// A provider image used as a boot source.
    Image,
    /// An SSH key registered with the provider.
    SshKey,
}

impl ResourceKind {
    /// Returns the stable lower-case name used in journals and reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::InstanceVolume => "instance-volume",
            Self::BlockVolume => "block-volume",
            Self::PublicIp => "public-ip",
            Self::Image => "image",
            Self::SshKey => "ssh-key",
        }
    }

    /// Reports whether the provider bills this kind of resource while it
    /// exists.
    ///
    /// Images and keys are borrowed references rather than run-owned
    /// allocations, so they are never deleted by cleanup even though they can
    /// carry a cost.
    #[must_use]
    pub const fn is_billable(self) -> bool {
        matches!(
            self,
            Self::Server | Self::InstanceVolume | Self::BlockVolume | Self::PublicIp
        )
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Storage API family that owns a volume.
///
/// Scaleway exposes local and Basic Block Storage volumes through the
/// Instances API, and SBS volumes through the Block Storage API. Cleanup must
/// call the family that actually owns the volume, because an identifier from
/// one family is unknown to the other.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorageFamily {
    /// Volume managed by the Instances API.
    Instance,
    /// Volume managed by the Block Storage API.
    Block,
}

/// Ownership disposition recorded for one resource.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Disposition {
    /// Allocated by this run; cleanup deletes it.
    Owned,
    /// Deliberately kept after the run; cleanup detaches but never deletes it.
    Retained,
    /// Referenced by this run but owned by someone else; cleanup detaches but
    /// never deletes it.
    Borrowed,
}

impl Disposition {
    /// Returns the stable lower-case name used in journals and reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owned => "owned",
            Self::Retained => "retained",
            Self::Borrowed => "borrowed",
        }
    }

    /// Reports whether cleanup may delete a resource with this disposition.
    #[must_use]
    pub const fn is_deletable(self) -> bool { matches!(self, Self::Owned) }
}

impl fmt::Display for Disposition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Zone and project that bound a ledger's resources.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AllocationScope {
    /// Availability zone every resource was allocated in.
    pub zone: String,
    /// Project that owns the resources.
    pub project_id: String,
}

impl AllocationScope {
    /// Creates a scope from its zone and project identifiers.
    #[must_use]
    pub fn new(zone: impl Into<String>, project_id: impl Into<String>) -> Self {
        Self {
            zone: zone.into(),
            project_id: project_id.into(),
        }
    }
}

/// Correlation tag shared by every provider resource of one allocation.
///
/// The tag is written to the provider before the first create request, so an
/// interrupted allocation can be rediscovered from the provider's own
/// metadata. A tag is always one filename component: it names the journal
/// file, so a value containing a path separator would write the journal
/// somewhere the recovery listing never scans.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct AllocationTag(String);

impl AllocationTag {
    /// Generates a fresh tag for a new allocation.
    #[must_use]
    pub fn generate() -> Self {
        Self(format!(
            "{ALLOCATION_TAG_PREFIX}{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    /// Parses a tag that carries the expected prefix and a non-empty suffix.
    ///
    /// The value must be a single filename component: path separators and
    /// `..` are rejected.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        let suffix = trimmed.strip_prefix(ALLOCATION_TAG_PREFIX)?;
        let has_path_syntax = suffix.contains(['/', '\\']) || suffix == "..";
        if suffix.is_empty() || has_path_syntax {
            return None;
        }
        Some(Self(trimmed.to_owned()))
    }

    /// Returns the tag text as written to provider metadata.
    ///
    /// # Returns
    ///
    /// The whole tag, including its prefix.
    #[must_use]
    pub const fn as_str(&self) -> &str { self.0.as_str() }

    /// Returns the part of the tag after exactly one prefix.
    ///
    /// The suffix is used in resource names, so a server and its allocation
    /// tag share one identifier.
    ///
    /// # Returns
    ///
    /// The remainder of the tag once the leading `mriya-alloc-` is removed,
    /// exactly as `parse` accepted it.
    #[must_use]
    pub fn suffix(&self) -> &str {
        self.0
            .strip_prefix(ALLOCATION_TAG_PREFIX)
            .unwrap_or(self.0.as_str())
    }
}

impl TryFrom<String> for AllocationTag {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).ok_or_else(|| format!("invalid allocation tag: {value}"))
    }
}

impl From<AllocationTag> for String {
    fn from(value: AllocationTag) -> Self { value.0 }
}

impl fmt::Display for AllocationTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A reference to one provider resource.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceRef {
    /// Kind of resource this reference names.
    kind: ResourceKind,
    /// Provider identifier.
    id: String,
    /// Availability zone that holds the resource.
    zone: String,
    /// Storage API family for volume references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    storage_family: Option<StorageFamily>,
    /// Whether a public IP is dynamic, for references that name an IP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ip_dynamic: Option<bool>,
}

impl ResourceRef {
    /// Creates a reference to a compute server.
    #[must_use]
    pub fn server(id: impl Into<String>, zone: impl Into<String>) -> Self {
        Self::new(ResourceKind::Server, id, zone)
    }

    /// Creates a reference to an Instances API volume.
    #[must_use]
    pub fn instance_volume(id: impl Into<String>, zone: impl Into<String>) -> Self {
        Self::new(ResourceKind::InstanceVolume, id, zone)
            .with_storage_family(StorageFamily::Instance)
    }

    /// Creates a reference to a Block Storage API volume.
    #[must_use]
    pub fn block_volume(id: impl Into<String>, zone: impl Into<String>) -> Self {
        Self::new(ResourceKind::BlockVolume, id, zone).with_storage_family(StorageFamily::Block)
    }

    /// Creates a reference to a public IP allocation.
    #[must_use]
    pub fn public_ip(id: impl Into<String>, zone: impl Into<String>, is_dynamic: bool) -> Self {
        Self {
            ip_dynamic: Some(is_dynamic),
            ..Self::new(ResourceKind::PublicIp, id, zone)
        }
    }

    /// Creates a reference to a borrowed image.
    #[must_use]
    pub fn image(id: impl Into<String>, zone: impl Into<String>) -> Self {
        Self::new(ResourceKind::Image, id, zone)
    }

    /// Creates a reference to a borrowed SSH key.
    #[must_use]
    pub fn ssh_key(id: impl Into<String>, zone: impl Into<String>) -> Self {
        Self::new(ResourceKind::SshKey, id, zone)
    }

    /// Creates a reference of `kind` with no family-specific details.
    #[must_use]
    pub fn new(kind: ResourceKind, id: impl Into<String>, zone: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
            zone: zone.into(),
            storage_family: None,
            ip_dynamic: None,
        }
    }

    /// Records the storage family a volume reference belongs to.
    #[must_use]
    pub const fn with_storage_family(mut self, family: StorageFamily) -> Self {
        self.storage_family = Some(family);
        self
    }

    /// Returns the resource kind.
    #[must_use]
    pub const fn kind(&self) -> ResourceKind { self.kind }

    /// Returns the provider identifier.
    #[must_use]
    pub const fn id(&self) -> &str { self.id.as_str() }

    /// Returns the availability zone.
    #[must_use]
    pub const fn zone(&self) -> &str { self.zone.as_str() }

    /// Returns the storage family for a volume reference.
    #[must_use]
    pub const fn storage_family(&self) -> Option<StorageFamily> { self.storage_family }

    /// Returns whether a public IP reference is dynamic.
    #[must_use]
    pub const fn ip_dynamic(&self) -> Option<bool> { self.ip_dynamic }

    /// Reports whether this reference names the resource identified by `kind`
    /// and `id`, ignoring the zone.
    #[must_use]
    pub fn matches(&self, kind: ResourceKind, id: &str) -> bool {
        self.kind == kind && self.id == id
    }
}

impl fmt::Display for ResourceRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}@{}", self.kind, self.id, self.zone)
    }
}

/// One resource recorded in a ledger, with the disposition claimed for it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LedgerEntry {
    /// The resource itself.
    pub resource: ResourceRef,
    /// How mriya came to hold it.
    pub disposition: Disposition,
}

impl LedgerEntry {
    /// Creates an entry from a resource and its disposition.
    #[must_use]
    pub const fn new(resource: ResourceRef, disposition: Disposition) -> Self {
        Self {
            resource,
            disposition,
        }
    }

    /// Reports whether cleanup may delete this entry's resource.
    #[must_use]
    pub const fn is_deletable(&self) -> bool { self.disposition.is_deletable() }
}

/// Point-in-time copy of a ledger, as persisted to a recovery journal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LedgerSnapshot {
    /// Correlation tag shared by the recorded resources.
    pub allocation: AllocationTag,
    /// Zone and project that bound the records.
    pub scope: AllocationScope,
    /// Resources recorded so far.
    pub entries: Vec<LedgerEntry>,
    /// Whether an allocation may exist without a recorded identifier.
    #[serde(default)]
    pub outcome_unknown: bool,
}
