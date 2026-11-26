//! A stateful loopback Scaleway API for lifecycle tests.
//!
//! Unlike the scripted [`super::http_stub::StubApi`], this fake keeps servers,
//! volumes, and IPs in memory and answers each request from that state. It lets
//! a test drive the production `create` and `cleanup` paths through several
//! round trips and then assert what the provider still holds, which a fixed
//! reply script cannot express.
//!
//! Failure knobs cover the cases the lifecycle must handle: an accepted create
//! whose response never arrives, a rejected delete, a rejected attachment, a
//! failed power-on, an omitted root volume, paginated lists, and a list that
//! returns an unrelated identifier.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

mod handlers;
mod state;
mod storage;

use handlers::{
    create_server,
    delete_server,
    get_server,
    list_servers,
    patch_server,
    server_action,
};
pub(super) use state::{FakeState, FakeVolume, Knobs};
use storage::{
    create_instance_volume,
    delete_block_volume,
    delete_instance_volume,
    delete_ip,
    get_block_volume,
    get_instance_volume,
    get_ip,
    list_block_volumes,
    list_instance_volumes,
    list_ips,
};

/// A running fake provider.
pub(super) struct FakeApi {
    /// Base URL for Instances API calls.
    instance_base: String,
    /// Base URL for Block Storage API calls.
    block_base: String,
    /// Shared provider state.
    state: Arc<Mutex<FakeState>>,
    /// Accept loop task.
    task: JoinHandle<()>,
}

impl FakeApi {
    /// Starts the fake on a loopback port.
    ///
    /// # Errors
    ///
    /// Returns the bind failure when no loopback port is available.
    pub(super) async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let state = Arc::new(Mutex::new(FakeState::default()));
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move { serve(listener, shared).await });
        Ok(Self {
            instance_base: format!("http://{address}/instance/v1"),
            block_base: format!("http://{address}/block/v1"),
            state,
            task,
        })
    }

    /// Returns the Instances API base URL.
    pub(super) fn instance_base(&self) -> &str { self.instance_base.as_str() }

    /// Returns the Block Storage API base URL.
    pub(super) fn block_base(&self) -> &str { self.block_base.as_str() }

    /// Locks the state, recovering a poisoned lock.
    pub(super) fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets a failure knob.
    pub(super) fn configure(&self, change: impl FnOnce(&mut Knobs)) {
        change(&mut self.lock().knobs);
    }

    /// Returns the recorded server identifiers.
    pub(super) fn server_ids(&self) -> Vec<String> { self.lock().servers.keys().cloned().collect() }

    /// Returns the recorded Instances API volume identifiers.
    pub(super) fn volume_ids(&self) -> Vec<String> {
        self.lock().instance_volumes.keys().cloned().collect()
    }

    /// Returns the recorded IP identifiers.
    pub(super) fn ip_ids(&self) -> Vec<String> { self.lock().ips.keys().cloned().collect() }

    /// Returns whether `volume_id` is attached to any server.
    pub(super) fn is_attached(&self, volume_id: &str) -> bool {
        self.lock()
            .servers
            .values()
            .any(|server| server.volumes.values().any(|v| v.id == volume_id))
    }
}

impl Drop for FakeApi {
    fn drop(&mut self) { self.task.abort(); }
}

/// Accepts connections until the task is aborted.
async fn serve(listener: TcpListener, state: Arc<Mutex<FakeState>>) {
    loop {
        let Ok((stream, _peer)) = listener.accept().await else {
            return;
        };
        let shared = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, shared).await {
                let _ = error;
            }
        });
    }
}

/// Serves requests on one connection until the client closes it.
///
/// Keep-alive matters here: closing after every response makes the client race
/// a pooled connection against the server's shutdown, and a lost request would
/// look like an ambiguous create that never reached the provider.
async fn handle_connection(mut stream: TcpStream, state: Arc<Mutex<FakeState>>) -> Result<()> {
    loop {
        let Some(request) = read_request(&mut stream).await? else {
            return Ok(());
        };
        let response = route(&state, &request);
        if response.status == 0 {
            // Modelling an accepted request whose response never arrives:
            // close the connection without writing anything.
            stream.shutdown().await?;
            return Ok(());
        }
        let text = format!(
            "HTTP/1.1 {} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            response.status,
            response.body.len(),
            response.body
        );
        stream.write_all(text.as_bytes()).await?;
        stream.flush().await?;
    }
}

/// A parsed request.
struct Request {
    /// Uppercase method.
    method: String,
    /// Path only.
    path: String,
    /// Decoded query pairs.
    query: BTreeMap<String, String>,
    /// JSON body, or null when absent.
    body: Value,
}

/// A scripted response.
struct Response {
    /// HTTP status.
    status: u16,
    /// JSON body.
    body: String,
}

impl Response {
    /// Builds a JSON response with `status`.
    fn json(status: u16, body: &Value) -> Self {
        Self {
            status,
            body: body.to_string(),
        }
    }

    /// Builds an empty response with `status`.
    fn empty(status: u16) -> Self {
        Self {
            status,
            body: String::new(),
        }
    }

    /// Builds the JSON error body Scaleway uses.
    fn error(status: u16, message: &str) -> Self {
        Self::json(status, &json!({"type": "error", "message": message}))
    }
}

/// Reads one complete HTTP request.
async fn read_request(stream: &mut TcpStream) -> Result<Option<Request>> {
    let mut buffer = Vec::new();
    loop {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Ok(None);
        }
        buffer.extend(chunk.iter().take(count));
        let text = std::str::from_utf8(&buffer)?;
        let Some((headers, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then_some(value.trim())
            })
            .map(str::parse::<usize>)
            .transpose()?
            .unwrap_or_default();
        if body.len() < length {
            continue;
        }
        let mut lines = headers.lines();
        let request_line = lines.next().context("request line missing")?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().context("method missing")?.to_owned();
        let target = parts.next().context("target missing")?;
        let (path, query_text) = target.split_once('?').unwrap_or((target, ""));
        let query = query_text
            .split('&')
            .filter(|pair| !pair.is_empty())
            .filter_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                Some((decode(key), decode(value)))
            })
            .collect();
        let payload = if body.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body)?
        };
        return Ok(Some(Request {
            method,
            path: path.to_owned(),
            query,
            body: payload,
        }));
    }
}

/// Decodes `%` escapes and `+` in a query component.
fn decode(value: &str) -> String {
    let replaced = value.replace('+', " ");
    let bytes = replaced.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(byte) = bytes.get(index) {
        if *byte == b'%' {
            let hex = bytes.get(index + 1..index + 3).unwrap_or_default();
            let text = std::str::from_utf8(hex).unwrap_or("");
            if let Ok(decoded) = u8::from_str_radix(text, 16) {
                out.push(decoded);
                index += 3;
                continue;
            }
        }
        out.push(*byte);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Routes one request against the fake state.
fn route(state: &Arc<Mutex<FakeState>>, request: &Request) -> Response {
    let mut locked = state.lock().unwrap_or_else(PoisonError::into_inner);
    locked.record(&request.method, &request.path);
    let segments = request
        .path
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    match (request.method.as_str(), segments.as_slice()) {
        ("POST", ["instance", "v1", "zones", zone, "servers"]) => {
            create_server(&mut locked, zone, request)
        }
        ("GET", ["instance", "v1", "zones", zone, "servers"]) => {
            list_servers(&locked, zone, request)
        }
        ("GET", ["instance", "v1", "zones", _zone, "servers", id]) => get_server(&locked, id),
        ("DELETE", ["instance", "v1", "zones", _zone, "servers", id]) => {
            delete_server(&mut locked, id)
        }
        ("POST", ["instance", "v1", "zones", _zone, "servers", id, "action"]) => {
            server_action(&mut locked, id, request)
        }
        ("PATCH", ["instance", "v1", "zones", _zone, "servers", id]) => {
            patch_server(&mut locked, id, request)
        }
        ("POST", ["instance", "v1", "zones", zone, "volumes"]) => {
            create_instance_volume(&mut locked, zone, request)
        }
        ("GET", ["instance", "v1", "zones", _zone, "volumes"]) => {
            list_instance_volumes(&locked, request)
        }
        ("GET", ["instance", "v1", "zones", _zone, "volumes", id]) => {
            get_instance_volume(&locked, id)
        }
        ("DELETE", ["instance", "v1", "zones", _zone, "volumes", id]) => {
            delete_instance_volume(&mut locked, id)
        }
        ("GET", ["instance", "v1", "zones", _zone, "ips"]) => list_ips(&locked, request),
        ("GET", ["instance", "v1", "zones", _zone, "ips", id]) => get_ip(&locked, id),
        ("DELETE", ["instance", "v1", "zones", _zone, "ips", id]) => delete_ip(&mut locked, id),
        ("GET", ["block", "v1", "zones", _zone, "volumes"]) => list_block_volumes(&locked, request),
        ("GET", ["block", "v1", "zones", _zone, "volumes", id]) => get_block_volume(&locked, id),
        ("DELETE", ["block", "v1", "zones", _zone, "volumes", id]) => {
            delete_block_volume(&mut locked, id)
        }
        _ => Response::error(404, "no route"),
    }
}
