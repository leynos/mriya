//! Types used by the Scaleway janitor.

use serde::Deserialize;

/// Scaleway instance server representation returned by `scw instance server list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ScwServer {
    pub(super) id: String,
    pub(super) zone: String,
    #[serde(default)]
    pub(super) tags: Vec<String>,
}

/// Scaleway Block Storage volume representation returned by `scw block volume list`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ScwVolume {
    pub(super) id: String,
    pub(super) zone: String,
    #[serde(default)]
    pub(super) tags: Vec<String>,
}
