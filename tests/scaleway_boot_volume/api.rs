//! Independent provider reads used by the live boot-volume check.
//!
//! Nothing here reuses the production capture path: the point of the check is
//! to read the account back through the public API and compare it with the
//! plan, so a bug in capture cannot hide a bug in what was created.

use std::time::Duration;

use mriya::{BootVolumeRequest, InstanceHandle, InstanceRequest, ScalewayConfig};

use crate::cases::Case;

/// Production Instances API base.
pub(super) const INSTANCE_API: &str = "https://api.scaleway.com/instance/v1";

/// Production Block Storage API base.
pub(super) const BLOCK_API: &str = "https://api.scaleway.com/block/v1";

/// Per-request budget for the independent reads this suite makes.
pub(super) const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Builds the request for one case, clearing the cache volume explicitly.
///
/// Clearing `volume_id` is deliberate: this check is about the boot disk, and
/// a configured cache volume would add an attachment this suite does not need
/// and would then have to clean up.
pub(super) fn build_request(
    config: &ScalewayConfig,
    case: &Case,
) -> Result<InstanceRequest, String> {
    let mut request = config
        .as_request()
        .map_err(|error| format!("cannot build a request: {error}"))?;
    request.instance_type.clone_from(&case.instance_type);
    request.volume_id = None;
    request.boot_volume = BootVolumeRequest::new(case.volume_type, case.size_gb);
    Ok(request)
}

/// Reads the server's boot entry straight from the Instances API.
pub(super) async fn read_server_root(
    client: &reqwest::Client,
    config: &ScalewayConfig,
    handle: &InstanceHandle,
) -> Result<(String, String, Option<u64>), String> {
    let url = format!(
        "{}/zones/{}/servers/{}",
        INSTANCE_API, handle.zone, handle.id
    );
    let body = get_json(client, &url, &config.secret_key)
        .await?
        .ok_or_else(|| format!("instance {} disappeared before it could be read", handle.id))?;
    let volumes = body
        .pointer("/server/volumes")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("instance {} reports no volumes object", handle.id))?;

    let entry = volumes
        .iter()
        .find(|(_, volume)| volume.get("boot") == Some(&serde_json::Value::Bool(true)))
        .or_else(|| volumes.get_key_value("0"))
        .map(|(_, volume)| volume)
        .ok_or_else(|| format!("instance {} reports no boot volume", handle.id))?;

    let id = entry
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("instance {} root volume has no id", handle.id))?;
    let volume_type = entry
        .get("volume_type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("instance {} root volume has no type", handle.id))?;
    let size = entry.get("size").and_then(serde_json::Value::as_u64);
    Ok((id.to_owned(), volume_type.to_owned(), size))
}

/// Reads an SBS volume from the Block Storage API.
pub(super) async fn read_block_volume(
    client: &reqwest::Client,
    config: &ScalewayConfig,
    zone: &str,
    volume_id: &str,
) -> Result<(u64, String), String> {
    let url = format!("{BLOCK_API}/zones/{zone}/volumes/{volume_id}");
    let body = get_json(client, &url, &config.secret_key)
        .await?
        .ok_or_else(|| format!("Block Storage has no volume {volume_id}"))?;
    let size = body
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("Block Storage volume {volume_id} reports no size"))?;
    let id = body
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("Block Storage volume {volume_id} reports no id"))?;
    Ok((size, id.to_owned()))
}

/// Fetches `url`, returning `None` for a 404 and an error otherwise.
pub(super) async fn get_json(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<Option<serde_json::Value>, String> {
    let response = client
        .get(url)
        .header("X-Auth-Token", token)
        .send()
        .await
        .map_err(|error| format!("request to {url} failed: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request to {url} returned {status}: {body}"));
    }
    response
        .json::<serde_json::Value>()
        .await
        .map(Some)
        .map_err(|error| format!("response from {url} was not JSON: {error}"))
}
