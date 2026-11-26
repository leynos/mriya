//! Newtypes and serde models for Scaleway lifecycle values.
//!
//! The newtypes keep provider identifiers and states from being mixed up as
//! bare strings. The deserialisable models mirror the Block Storage v1 `Volume`
//! object and its volume-types list; they are local rather than SDK types
//! because `scaleway-rs` does not model the Block Storage API.

use std::ops::Deref;

use serde::Deserialize;

use crate::volume::{VolumeReference, VolumeState, VolumeStatus};

/// Defines a string-backed newtype with `new`, `as_str`, conversions from
/// `String` and `&str`, `AsRef<str>`, and `Deref<Target = str>`.
///
/// Each generated type keeps provider identifiers and states from being
/// mixed up as bare strings.
///
/// # Examples
///
/// ```ignore
/// newtype!(InstanceId);
///
/// // Both conversions yield the same string, and the newtype derefs to it.
/// let id = InstanceId::from("11111111-1111-1111-1111-111111111111");
/// assert_eq!(id.as_str(), "11111111-1111-1111-1111-111111111111");
/// assert_eq!(&*id, "11111111-1111-1111-1111-111111111111");
///
/// // The type prevents mixing an identifier into a field that wants a state.
/// let state = InstanceState::from(String::from("running"));
/// assert_eq!(state.as_str(), "running");
/// ```
macro_rules! newtype {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub(crate) struct $name(String);

        impl $name {
            #[expect(dead_code, reason = "builder retained for future ergonomics")]
            pub(crate) fn new(value: impl Into<String>) -> Self { Self(value.into()) }
            pub(crate) const fn as_str(&self) -> &str { self.0.as_str() }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self { Self(value) }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self { Self(value.to_owned()) }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str { self.as_str() }
        }

        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &Self::Target { self.as_str() }
        }
    };
}

newtype!(InstanceId);
newtype!(Zone);
newtype!(InstanceState);
newtype!(Action);

/// Block Storage volume as returned by the Block v1 API.
///
/// The create, get and list endpoints return this object directly; unlike the
/// Instances API it is not wrapped in a `volume` key.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SbsVolume {
    /// Volume UUID.
    pub(crate) id: String,
    /// Human-friendly name; absent only in malformed provider responses.
    ///
    /// Retained because the provider contract defines it; the orchestrator
    /// acts on identifiers and status only.
    #[serde(default)]
    #[expect(dead_code, reason = "provider contract field kept for completeness")]
    pub(crate) name: Option<String>,
    /// Provider volume type, such as `sbs_5k` or the legacy `b_ssd`.
    #[serde(rename = "type")]
    pub(crate) volume_type: String,
    /// Size in bytes.
    pub(crate) size: u64,
    /// Owning project UUID.
    #[serde(default)]
    pub(crate) project_id: String,
    /// Zone that holds the volume.
    pub(crate) zone: String,
    /// Provider status string, kept as text so unknown values survive.
    pub(crate) status: String,
    /// Tags assigned to the volume.
    #[serde(default)]
    #[expect(dead_code, reason = "provider contract field kept for completeness")]
    pub(crate) tags: Vec<String>,
    /// Resources that currently reference the volume.
    #[serde(default)]
    pub(crate) references: Vec<SbsVolumeReference>,
    /// Commercial specifications, including the storage class and IOPS.
    #[serde(default)]
    pub(crate) specs: Option<SbsVolumeSpecs>,
}

impl SbsVolume {
    /// Converts the provider object into the orchestrator-facing state.
    pub(crate) fn into_state(self) -> VolumeState {
        VolumeState {
            id: self.id,
            zone: self.zone,
            project_id: self.project_id,
            status: VolumeStatus::from_provider(&self.status),
            volume_type: self.volume_type,
            storage_class: self.specs.and_then(|specs| specs.class),
            size_bytes: self.size,
            references: self
                .references
                .into_iter()
                .map(SbsVolumeReference::into_reference)
                .collect(),
        }
    }
}

/// Commercial specifications attached to a Block Storage volume.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SbsVolumeSpecs {
    /// Provisioned IOPS, for example `5000`.
    #[serde(default)]
    #[expect(dead_code, reason = "provider contract field kept for completeness")]
    pub(crate) perf_iops: Option<u32>,
    /// Storage class, `sbs` or `bssd`.
    #[serde(default)]
    pub(crate) class: Option<String>,
}

/// One resource referencing a Block Storage volume.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SbsVolumeReference {
    /// Reference UUID.
    #[serde(default)]
    pub(crate) id: Option<String>,
    /// Product resource type, such as `instance_server`.
    #[serde(default)]
    pub(crate) product_resource_type: Option<String>,
    /// Identifier of the referencing resource.
    #[serde(default)]
    pub(crate) product_resource_id: Option<String>,
    /// Reference status, such as `attached`.
    #[serde(default)]
    pub(crate) status: Option<String>,
}

impl SbsVolumeReference {
    /// Converts the provider reference into the orchestrator-facing value.
    fn into_reference(self) -> VolumeReference {
        VolumeReference {
            id: self.id.unwrap_or_default(),
            resource_type: self.product_resource_type.unwrap_or_default(),
            resource_id: self.product_resource_id.unwrap_or_default(),
            status: self.status.unwrap_or_default(),
        }
    }
}

/// Response from the Block v1 volume-types endpoint.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct VolumeTypesResponse {
    /// Volume types currently in stock for the zone.
    #[serde(default)]
    pub(crate) volume_types: Vec<SbsVolumeType>,
}

/// One volume type listed for a zone.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SbsVolumeType {
    /// Provider type name, such as `sbs_5k`.
    #[serde(rename = "type")]
    pub(crate) volume_type: String,
}
