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
