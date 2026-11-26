//! Volume attachment payloads and server volume-map verification.
//!
//! Scaleway attaches a Block Storage volume to a server through the dedicated
//! `attach-volume` endpoint. Each request names one volume, so the provider
//! keeps the rest of the server's volume map. The response still carries the
//! complete map, and Mriya verifies it: the root entry and every unrelated
//! entry must survive, and the cache entry must be present with the
//! `sbs_volume` type. Detachment verifies the inverse.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// Provider volume type used when attaching a Block Storage volume.
pub(crate) const SBS_VOLUME_TYPE: &str = "sbs_volume";

/// Request body for the attach-volume endpoint.
///
/// # Examples
///
/// ```ignore
/// let json = serde_json::to_value(AttachServerVolumeRequest::new("vol-1"))?;
/// assert_eq!(json, serde_json::json!({"volume_id": "vol-1", "volume_type": "sbs_volume"}));
/// ```
#[derive(Clone, Debug, Serialize)]
pub(crate) struct AttachServerVolumeRequest<'a> {
    /// Volume identifier to attach.
    pub(crate) volume_id: &'a str,
    /// Provider volume type; always `sbs_volume` here.
    pub(crate) volume_type: &'static str,
}

impl<'a> AttachServerVolumeRequest<'a> {
    /// Builds the request for `volume_id`.
    pub(crate) const fn new(volume_id: &'a str) -> Self {
        Self {
            volume_id,
            volume_type: SBS_VOLUME_TYPE,
        }
    }
}

/// Request body for the detach-volume endpoint.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct DetachServerVolumeRequest<'a> {
    /// Volume identifier to detach.
    pub(crate) volume_id: &'a str,
}

/// Response wrapper shared by the attach and detach endpoints.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerVolumesResponse {
    /// Server carrying the resulting volume map.
    pub(crate) server: ServerVolumesBody,
}

/// Server fields read from the attach or detach response.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerVolumesBody {
    /// Volume map keyed by index, with `"0"` holding the root volume.
    #[serde(default)]
    pub(crate) volumes: ServerVolumeMap,
}

/// Server volume map keyed by index.
pub(crate) type ServerVolumeMap = BTreeMap<String, ServerVolumeEntry>;

/// One entry in a server's volume map.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ServerVolumeEntry {
    /// Provider volume identifier.
    pub(crate) id: String,
    /// Provider volume type, absent in some abbreviated payloads.
    #[serde(default)]
    pub(crate) volume_type: Option<String>,
}

/// Snapshot of the volume map taken before an attachment.
pub(crate) type ExpectedVolumeMap = BTreeMap<String, ServerVolumeEntry>;

/// Builds the expected map from the server returned by the create call.
///
/// The SDK does not model the per-volume `boot` flag, so the comparison covers
/// the keys, identifiers and volume types; that is enough to prove the
/// attachment did not replace or drop an unrelated volume.
pub(crate) fn expected_map(
    volumes: &HashMap<String, scaleway_rs::ScalewayImageExtraVolume>,
) -> ExpectedVolumeMap {
    volumes
        .iter()
        .map(|(index, volume)| {
            (
                index.clone(),
                ServerVolumeEntry {
                    id: volume.id.clone(),
                    volume_type: Some(volume.volume_type.clone()),
                },
            )
        })
        .collect()
}

/// Verifies that an attachment added the cache without disturbing the map.
///
/// # Errors
///
/// Returns a human-readable description of the first violated invariant: a
/// missing or changed expected entry, a missing cache entry, or a cache entry
/// that is not typed `sbs_volume`.
///
/// # Examples
///
/// ```ignore
/// let expected = expected_map(&server.volumes.volumes);
/// let returned = response.server.volumes;
/// verify_attached(&returned, "vol-cache", &expected)?;
/// ```
pub(crate) fn verify_attached(
    returned: &ServerVolumeMap,
    volume_id: &str,
    expected: &ExpectedVolumeMap,
) -> Result<(), String> {
    verify_expected_entries(returned, expected)?;
    verify_cache_entry(returned, volume_id)
}

/// Verifies that every pre-attachment entry survives unchanged.
fn verify_expected_entries(
    returned: &ServerVolumeMap,
    expected: &ExpectedVolumeMap,
) -> Result<(), String> {
    for (index, before) in expected {
        let Some(after) = returned.get(index) else {
            return Err(format!(
                "volume index {index} ({}) disappeared after attachment",
                before.id
            ));
        };
        if after.id != before.id {
            return Err(format!(
                "volume index {index} changed from {} to {}",
                before.id, after.id
            ));
        }
    }
    Ok(())
}

/// Verifies that the cache is present once, off the root index, and typed SBS.
fn verify_cache_entry(returned: &ServerVolumeMap, volume_id: &str) -> Result<(), String> {
    let mut matches = returned.iter().filter(|(_, entry)| entry.id == volume_id);
    let Some((index, entry)) = matches.next() else {
        return Err(format!(
            "cache volume {volume_id} is absent from the returned volume map"
        ));
    };
    if matches.next().is_some() {
        return Err(format!(
            "cache volume {volume_id} appears at more than one volume index"
        ));
    }
    if index.as_str() == "0" {
        return Err(String::from(
            "cache volume was attached at the root index \"0\"",
        ));
    }
    match entry.volume_type.as_deref() {
        Some(SBS_VOLUME_TYPE) => Ok(()),
        Some(other) => Err(format!(
            "cache volume {volume_id} was attached as '{other}', not '{SBS_VOLUME_TYPE}'"
        )),
        None => Err(format!(
            "cache volume {volume_id} was attached without a volume type"
        )),
    }
}

/// Verifies that a detachment removed the cache and kept the rest of the map.
///
/// # Errors
///
/// Returns a human-readable description when the cache is still present, when
/// the root is missing, or when an unrelated entry changed or disappeared.
///
/// # Examples
///
/// ```ignore
/// let expected = expected_map(&instance.volumes.volumes);
/// verify_detached(&response.server.volumes, "vol-cache", &expected)?;
/// ```
pub(crate) fn verify_detached(
    returned: &ServerVolumeMap,
    volume_id: &str,
    expected: &ExpectedVolumeMap,
) -> Result<(), String> {
    verify_cache_removed(returned, volume_id)?;
    verify_surviving_entries(returned, expected, volume_id)?;
    verify_root_present(returned)
}

/// Rejects a detachment response that still contains the cache entry.
fn verify_cache_removed(returned: &ServerVolumeMap, volume_id: &str) -> Result<(), String> {
    if returned.values().any(|entry| entry.id == volume_id) {
        return Err(format!(
            "cache volume {volume_id} is still present after detachment"
        ));
    }
    Ok(())
}

/// Verifies that every unrelated entry survives the detachment unchanged.
fn verify_surviving_entries(
    returned: &ServerVolumeMap,
    expected: &ExpectedVolumeMap,
    volume_id: &str,
) -> Result<(), String> {
    for (index, before) in expected {
        if before.id == volume_id {
            continue;
        }
        let Some(after) = returned.get(index) else {
            return Err(if index.as_str() == "0" {
                String::from("root volume is missing from the map after detachment")
            } else {
                format!(
                    "volume index {index} ({}) disappeared after detachment",
                    before.id
                )
            });
        };
        if after.id != before.id {
            return Err(format!(
                "volume index {index} changed from {} to {} during detachment",
                before.id, after.id
            ));
        }
    }
    Ok(())
}

/// Verifies that the returned map still has a non-blank root entry.
fn verify_root_present(returned: &ServerVolumeMap) -> Result<(), String> {
    match returned.get("0") {
        Some(root) if !root.id.trim().is_empty() => Ok(()),
        Some(_) => Err(String::from(
            "root volume identifier is blank after detachment",
        )),
        None => Err(String::from(
            "root volume is missing from the map after detachment",
        )),
    }
}
