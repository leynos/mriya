//! Volume, IP, and Block Storage handlers for the fake provider.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::{
    Request,
    Response,
    state::{FakeIp, FakeServer, FakeState, FakeVolume, page_items},
};

/// Creates an Instances API volume.
pub(super) fn create_instance_volume(
    state: &mut FakeState,
    zone: &str,
    request: &Request,
) -> Response {
    let id = state.next_id("vol");
    let volume = FakeVolume {
        id: id.clone(),
        zone: zone.to_owned(),
        volume_type: String::from("b_ssd"),
        attached_to: None,
    };
    state.instance_volumes.insert(id, volume.clone());
    let _ = request;
    Response::json(
        201,
        &json!({ "volume": { "id": volume.id, "zone": volume.zone } }),
    )
}

/// Lists Instances API volumes for one page.
pub(super) fn list_instance_volumes(state: &FakeState, request: &Request) -> Response {
    let page = request
        .query
        .get("page")
        .and_then(|page| page.parse::<usize>().ok())
        .unwrap_or(1);
    let volumes = state.instance_volumes.values().collect::<Vec<_>>();
    let selected = page_items(&volumes, page, state.knobs.page_size);
    Response::json(
        200,
        &json!({
            "volumes": selected
                .into_iter()
                .map(|volume| volume_json(volume, is_attached(volume, &state.servers)))
                .collect::<Vec<_>>(),
        }),
    )
}

/// Returns one Instances API volume.
pub(super) fn get_instance_volume(state: &FakeState, id: &str) -> Response {
    state.instance_volumes.get(id).map_or_else(
        || Response::error(404, "no such volume"),
        |volume| {
            Response::json(
                200,
                &json!({ "volume": volume_json(volume, is_attached(volume, &state.servers)) }),
            )
        },
    )
}

/// Deletes one Instances API volume.
pub(super) fn delete_instance_volume(state: &mut FakeState, id: &str) -> Response {
    match state.instance_volumes.remove(id) {
        Some(_) => Response::empty(204),
        None => Response::error(404, "no such volume"),
    }
}

/// Lists IPs for one page.
pub(super) fn list_ips(state: &FakeState, request: &Request) -> Response {
    let page = request
        .query
        .get("page")
        .and_then(|page| page.parse::<usize>().ok())
        .unwrap_or(1);
    let ips = state.ips.values().collect::<Vec<_>>();
    let selected = page_items(&ips, page, state.knobs.page_size);
    Response::json(
        200,
        &json!({ "ips": selected.into_iter().map(|ip| ip_json(ip)).collect::<Vec<_>>() }),
    )
}

/// Returns one IP.
pub(super) fn get_ip(state: &FakeState, id: &str) -> Response {
    state.ips.get(id).map_or_else(
        || Response::error(404, "no such ip"),
        |ip| Response::json(200, &json!({ "ip": ip_json(ip) })),
    )
}

/// Deletes one IP.
pub(super) fn delete_ip(state: &mut FakeState, id: &str) -> Response {
    match state.ips.remove(id) {
        Some(_) => Response::empty(204),
        None => Response::error(404, "no such ip"),
    }
}

/// Lists Block Storage volumes for one page.
pub(super) fn list_block_volumes(state: &FakeState, request: &Request) -> Response {
    let page = request
        .query
        .get("page")
        .and_then(|page| page.parse::<usize>().ok())
        .unwrap_or(1);
    let volumes = state.block_volumes.values().collect::<Vec<_>>();
    let selected = page_items(&volumes, page, state.knobs.page_size);
    Response::json(
        200,
        &json!({
            "volumes": selected
                .into_iter()
                .map(|volume| json!({
                    "id": volume.id,
                    "zone": volume.zone,
                    "status": if volume.attached_to.is_some() { "in_use" } else { "available" },
                }))
                .collect::<Vec<_>>(),
        }),
    )
}

/// Returns one Block Storage volume.
pub(super) fn get_block_volume(state: &FakeState, id: &str) -> Response {
    state.block_volumes.get(id).map_or_else(
        || Response::error(404, "no such block volume"),
        |volume| {
            Response::json(
                200,
                &json!({
                    "volume": {
                        "id": volume.id,
                        "zone": volume.zone,
                        "status": if volume.attached_to.is_some() { "in_use" } else { "available" },
                    },
                }),
            )
        },
    )
}

/// Deletes one Block Storage volume.
pub(super) fn delete_block_volume(state: &mut FakeState, id: &str) -> Response {
    match state.block_volumes.remove(id) {
        Some(_) => Response::empty(204),
        None => Response::error(404, "no such block volume"),
    }
}

/// Serializes a server.
pub(super) fn server_json(server: &FakeServer) -> Value {
    let volumes = server
        .volumes
        .iter()
        .map(|(index, volume)| (index.clone(), volume_json(volume, true)))
        .collect::<Map<String, Value>>();
    json!({
        "id": server.id,
        "zone": server.zone,
        "project": server.project,
        "state": server.state,
        "tags": server.tags,
        "allowed_actions": ["poweron", "poweroff"],
        "public_ips": server.ips.iter().map(ip_json).collect::<Vec<_>>(),
        "volumes": volumes,
    })
}

/// Reports whether `volume` is attached to a server the fake still holds.
pub(super) fn is_attached(volume: &FakeVolume, servers: &BTreeMap<String, FakeServer>) -> bool {
    volume
        .attached_to
        .as_ref()
        .is_some_and(|id| servers.contains_key(id))
}

/// Serializes a volume, describing its attachment as `attached` says.
pub(super) fn volume_json(volume: &FakeVolume, attached: bool) -> Value {
    let server = volume
        .attached_to
        .as_ref()
        .filter(|_| attached)
        .map(|id| json!({ "id": id }));
    json!({
        "id": volume.id,
        "zone": volume.zone,
        "volume_type": volume.volume_type,
        "state": if attached { "attached" } else { "available" },
        "server": server,
        "tags": [],
    })
}

/// Serializes an IP.
pub(super) fn ip_json(ip: &FakeIp) -> Value {
    json!({
        "id": ip.id,
        "address": ip.address,
        "dynamic": ip.dynamic,
    })
}
