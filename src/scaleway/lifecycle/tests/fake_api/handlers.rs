//! Server handlers for the fake provider.
//!
//! Each function answers one server route from [`super::FakeState`], applying
//! the requested mutation and returning the response the provider would send.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{
    Request,
    Response,
    state::{FakeIp, FakeServer, FakeState, FakeVolume, page_items},
    storage::server_json,
};

/// Creates a stopped server with a root volume.
pub(super) fn create_server(state: &mut FakeState, zone: &str, request: &Request) -> Response {
    // A connection the fake closes without a response can be retried by the
    // client, so a repeat of the *same* allocation must not create a second
    // server; the retry reconciles against the server the first attempt made.
    if state.knobs.drop_create_response
        && allocation_tag(request).is_some_and(|tag| {
            state
                .servers
                .values()
                .any(|server| server.tags.iter().any(|existing| existing == tag))
        })
    {
        return Response::empty(0);
    }
    let id = state.next_id("srv");
    let mut volumes = BTreeMap::new();
    if !state.knobs.omit_root_volume {
        let root_id = state.next_id("vol");
        let root = FakeVolume {
            id: root_id.clone(),
            zone: zone.to_owned(),
            volume_type: String::from("b_ssd"),
            attached_to: Some(id.clone()),
        };
        state.instance_volumes.insert(root_id, root.clone());
        volumes.insert(String::from("0"), root);
    }
    let tags = request
        .body
        .get("tags")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        });
    let project = request
        .body
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let server = FakeServer {
        id: id.clone(),
        zone: zone.to_owned(),
        project,
        state: String::from("stopped"),
        tags,
        volumes,
        ips: Vec::new(),
    };
    state.servers.insert(id, server.clone());
    if state.knobs.drop_create_response {
        // The create is applied; the caller will see a closed connection.
        return Response::empty(0);
    }
    Response::json(201, &json!({ "server": server_json(&server) }))
}

/// Returns the allocation tag named by a create request, if any.
pub(super) fn allocation_tag(request: &Request) -> Option<&str> {
    request
        .body
        .get("tags")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .find(|tag| tag.starts_with(crate::ownership::ALLOCATION_TAG_PREFIX))
}

/// Lists servers for one page.
pub(super) fn list_servers(state: &FakeState, _zone: &str, request: &Request) -> Response {
    let page = request
        .query
        .get("page")
        .and_then(|page| page.parse::<usize>().ok())
        .unwrap_or(1);
    let page_size = state.knobs.page_size;
    let servers = state.servers.values().collect::<Vec<_>>();
    let selected = page_items(&servers, page, page_size)
        .into_iter()
        .map(|server| {
            if state.knobs.wrong_list_id {
                let mut value = server_json(server);
                if let Some(object) = value.as_object_mut() {
                    object.insert(String::from("id"), json!(format!("{}-other", server.id)));
                }
                value
            } else {
                server_json(server)
            }
        })
        .collect::<Vec<_>>();
    Response::json(200, &json!({ "servers": selected }))
}

/// Returns one server.
pub(super) fn get_server(state: &FakeState, id: &str) -> Response {
    state.servers.get(id).map_or_else(
        || Response::error(404, "no such server"),
        |server| Response::json(200, &json!({ "server": server_json(server) })),
    )
}

/// Deletes one server and releases its dynamic IPs.
pub(super) fn delete_server(state: &mut FakeState, id: &str) -> Response {
    if state.knobs.deny_server_delete {
        return Response::error(403, "denied");
    }
    match state.servers.remove(id) {
        Some(server) => {
            for ip in server.ips {
                if ip.dynamic {
                    state.ips.remove(&ip.id);
                }
            }
            Response::empty(204)
        }
        None => Response::error(404, "no such server"),
    }
}

/// Applies a power action.
pub(super) fn server_action(state: &mut FakeState, id: &str, request: &Request) -> Response {
    let action = request
        .body
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let knobs = state.knobs.clone();
    if !state.servers.contains_key(id) {
        return Response::error(404, "no such server");
    }
    match action {
        "poweron" => {
            if knobs.fail_power_on {
                return Response::error(400, "poweron rejected");
            }
            let ip = FakeIp {
                id: state.next_id("ip"),
                address: String::from("192.0.2.10"),
                dynamic: true,
            };
            state.ips.insert(ip.id.clone(), ip.clone());
            if let Some(server) = state.servers.get_mut(id) {
                server.state = String::from("running");
                server.ips.push(ip);
            }
        }
        "poweroff" => {
            if let Some(server) = state.servers.get_mut(id) {
                server.state = String::from("stopped");
            }
        }
        _ => return Response::error(400, "unknown action"),
    }
    Response::json(202, &json!({}))
}

/// Replaces the server's volume map.
///
/// Validation finishes before any attachment state changes, so a rejected
/// request leaves the provider untouched.
pub(super) fn patch_server(state: &mut FakeState, id: &str, request: &Request) -> Response {
    if state.knobs.deny_volume_attach {
        return Response::error(403, "denied");
    }
    let Some(volumes) = request.body.get("volumes").and_then(Value::as_object) else {
        return Response::error(400, "volumes missing");
    };
    if !state.servers.contains_key(id) {
        return Response::error(404, "no such server");
    }
    let mut replacement: BTreeMap<String, FakeVolume> = BTreeMap::new();
    for (index, entry) in volumes {
        let Some(volume_id) = entry.get("id").and_then(Value::as_str) else {
            return Response::error(400, "volume id missing");
        };
        let Some(volume) = state.instance_volumes.get(volume_id) else {
            return Response::error(404, "no such volume");
        };
        replacement.insert(index.clone(), volume.clone());
    }

    let Some(server) = state.servers.get_mut(id) else {
        return Response::error(404, "no such server");
    };
    // Clear the attachment only for volumes the replacement drops; a volume
    // still present keeps its server reference.
    let kept = replacement
        .values()
        .map(|volume| volume.id.as_str())
        .collect::<Vec<_>>();
    for previous in server.volumes.values() {
        if kept.contains(&previous.id.as_str()) {
            continue;
        }
        if let Some(volume) = state.instance_volumes.get_mut(&previous.id) {
            volume.attached_to = None;
        }
    }
    for volume in replacement.values_mut() {
        volume.attached_to = Some(id.to_owned());
        state
            .instance_volumes
            .insert(volume.id.clone(), volume.clone());
    }
    server.volumes = replacement;
    Response::json(200, &json!({}))
}
