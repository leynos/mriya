//! Types used by the Scaleway janitor.

use serde::Deserialize;

/// Scaleway instance server representation returned by `scw instance server list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ScwServer {
    /// Provider identifier of the server.
    pub(super) id: String,
    /// Zone the server lives in, needed to address it for deletion.
    pub(super) zone: String,
    /// Tags on the server; absent in the JSON means none.
    #[serde(default)]
    pub(super) tags: Vec<String>,
    /// Volumes attached to the server, keyed by index. `"0"` is the root.
    #[serde(default)]
    pub(super) volumes: std::collections::BTreeMap<String, ScwServerVolume>,
}

/// A volume attached to a server, as the server listing reports it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(super) struct ScwServerVolume {
    /// Provider identifier of the volume.
    pub(super) id: String,
    /// Zone the volume lives in.
    #[serde(default)]
    pub(super) zone: String,
}

/// Scaleway Block Storage volume representation returned by `scw block volume list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ScwVolume {
    /// Provider identifier of the volume.
    pub(super) id: String,
    /// Zone the volume lives in, needed to address it for deletion.
    pub(super) zone: String,
    /// Tags on the volume; absent in the JSON means none.
    #[serde(default)]
    pub(super) tags: Vec<String>,
}
