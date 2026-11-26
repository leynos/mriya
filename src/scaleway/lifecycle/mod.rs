//! Instance lifecycle helpers for the Scaleway backend.

use std::{sync::LazyLock, time::Duration};

mod block_volumes;
mod create;
mod http;
mod image;
mod instances;
mod ips;
pub(in crate::scaleway) mod models;
mod power;
mod reconcile;
pub(in crate::scaleway) mod records;
mod volume_attach;
mod volume_create;
mod volumes;
mod wait;

use crate::scaleway::types::{Action, InstanceId, InstanceState};

/// Per-request timeout for direct HTTP calls to the provider.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Production base URL for the Instances API, used as the default for
/// `ScalewayBackend::instance_api_base`.
pub(super) const SCALEWAY_INSTANCE_API_BASE: &str = "https://api.scaleway.com/instance/v1";
/// Production base URL for the Block Storage API, used as the default for
/// `ScalewayBackend::block_api_base`.
///
/// SBS volumes live in their own API and identifier namespace; the Instances
/// API cannot read or delete them.
pub(super) const SCALEWAY_BLOCK_API_BASE: &str = "https://api.scaleway.com/block/v1";

/// Shared HTTP client for direct provider calls.
///
/// If the timeout-configured builder fails, it falls back to a default
/// client without the timeout, so callers that must be bounded also set a
/// per-request timeout.
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Point-in-time view of a provider instance, reduced to the fields the
/// lifecycle logic reads when deciding whether to power on or whether the
/// instance is reachable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceSnapshot {
    /// Provider identifier of the instance.
    pub(crate) id: InstanceId,
    /// Provider-reported state, such as `running` or `stopped`.
    pub(crate) state: InstanceState,
    /// Actions the provider currently permits, such as `poweron`.
    pub(crate) allowed_actions: Vec<Action>,
    /// Public IPv4 address as reported by the provider, if one is assigned.
    pub(crate) public_ip: Option<String>,
}

#[cfg(test)]
mod tests;
