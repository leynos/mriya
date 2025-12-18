//! Types used by the Scaleway janitor.

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(super) struct ScwServer {
    pub(super) id: String,
    pub(super) zone: String,
    #[serde(default)]
    pub(super) tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(super) struct ScwVolume {
    pub(super) id: String,
    pub(super) zone: String,
    #[serde(default)]
    pub(super) tags: Vec<String>,
}
