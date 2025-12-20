//! Instance lifecycle helpers for the Scaleway backend.

use std::sync::LazyLock;
use std::time::Duration;

mod create;
mod image;
mod volume_attach;
mod wait;

use crate::scaleway::types::{Action, InstanceId, InstanceState};

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const SCALEWAY_INSTANCE_API_BASE: &str = "https://api.scaleway.com/instance/v1";

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceSnapshot {
    pub(crate) id: InstanceId,
    pub(crate) state: InstanceState,
    pub(crate) allowed_actions: Vec<Action>,
    pub(crate) public_ip: Option<String>,
}

#[cfg(test)]
mod tests;
