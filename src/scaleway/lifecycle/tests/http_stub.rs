//! Loopback HTTP scripts for lifecycle boundary tests.
//!
//! Every response closes its connection, and callers await the task with a
//! bounded timeout so skipped delegation cannot leave a test hanging. The
//! request readers live here beside the recorder so each suite asserts the
//! captured text the same way.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

use crate::scaleway::{ScalewayBackend, ScalewayBackendError};

pub(super) struct StubApi {
    pub(super) base_url: String,
    pub(super) block_base_url: String,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<Vec<String>>>,
}

pub(super) struct Reply {
    pub(super) status: u16,
    pub(super) body: String,
}

impl StubApi {
    /// Starts a loopback API returning each scripted response in order.
    ///
    /// For example, two empty-list replies serve exactly two requests, whose
    /// complete request text is returned by `finish` for assertions. The
    /// listener keeps accepting after the script is exhausted so a request
    /// sent by a buggy cleanup path is recorded and fails the test instead of
    /// being silently refused.
    pub(super) async fn start(replies: Vec<Reply>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (stop, stop_rx) = oneshot::channel();
        let task = tokio::spawn(serve(listener, replies, stop_rx));
        Ok(Self {
            base_url: format!("http://{address}/instance/v1"),
            block_base_url: format!("http://{address}/block/v1"),
            stop: Some(stop),
            task,
        })
    }

    /// Stops the stub and returns the captured requests.
    ///
    /// # Errors
    ///
    /// Fails when a scripted reply was not consumed, or when the lifecycle sent
    /// a request after the script ended. Both mean the production path differs
    /// from the script the test wrote.
    pub(super) async fn finish(mut self) -> Result<Vec<String>> {
        if let Some(stop) = self.stop.take() {
            // The receiver is gone only when the stub task already finished.
            let _sent = stop.send(());
        }
        tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .context("lifecycle did not consume the HTTP script")?
            .context("HTTP script task failed")?
    }
}

/// Serves the script, then records any further request until stopped.
async fn serve(
    listener: TcpListener,
    replies: Vec<Reply>,
    stop: oneshot::Receiver<()>,
) -> Result<Vec<String>> {
    tokio::pin!(stop);
    let mut requests = Vec::new();
    for reply in replies {
        let Some(mut stream) = next_stream(&listener, &mut stop).await? else {
            bail!("lifecycle did not consume the HTTP script");
        };
        let request = read_request(&mut stream).await?;
        write_reply(&mut stream, &reply, &request).await?;
        requests.push(request);
    }
    if let Some(mut stream) = next_stream(&listener, &mut stop).await? {
        let request = read_request(&mut stream).await?;
        write_reply(
            &mut stream,
            &Reply {
                status: 500,
                body: String::from("{}"),
            },
            &request,
        )
        .await?;
        bail!("lifecycle sent a request after the script ended: {request}");
    }
    Ok(requests)
}

/// Waits for the next connection, or for the caller to stop the stub.
async fn next_stream(
    listener: &TcpListener,
    stop: &mut std::pin::Pin<&mut oneshot::Receiver<()>>,
) -> Result<Option<TcpStream>> {
    tokio::select! {
        accepted = listener.accept() => {
            let (stream, _) = accepted?;
            Ok(Some(stream))
        }
        _ = &mut **stop => Ok(None),
    }
}

/// Replaces a `{{name}}` placeholder with the request's `name` filter.
///
/// A reply that has to echo a value the test cannot know in advance, such as a
/// generated server name, uses the placeholder instead of a copied fixture.
fn substitute_request(body: &str, request: &str) -> String {
    if !body.contains("{{name}}") {
        return body.to_owned();
    }
    let name = request
        .split_whitespace()
        .nth(1)
        .and_then(|target| reqwest::Url::parse(&format!("http://localhost{target}")).ok())
        .and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "name")
                .map(|(_, value)| value.into_owned())
        })
        .unwrap_or_default();
    body.replace("{{name}}", &name)
}

/// Writes one scripted response and closes the connection.
async fn write_reply(stream: &mut TcpStream, reply: &Reply, request: &str) -> Result<()> {
    let body = substitute_request(&reply.body, request);
    let response = format!(
        "HTTP/1.1 {} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        body.len(),
        body,
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

/// Runs `call` against a loopback API scripting `replies` in order.
///
/// The backend is pointed at the stub's Instances and Block Storage bases
/// before `call` runs, so a wrapper that formats its URL from either field
/// reaches the stub while one that hard-codes the production host fails to
/// connect. Both the call's result and the captured requests are returned,
/// letting each suite assert on what the provider would have received. For
/// example, a tier listing followed by a 200 create yields two captured
/// requests and `Ok(handle)`.
pub(super) async fn with_stub<T, F>(
    mut backend: ScalewayBackend,
    replies: Vec<Reply>,
    call: F,
) -> Result<(Result<T, ScalewayBackendError>, Vec<String>)>
where
    F: AsyncFnOnce(&ScalewayBackend) -> Result<T, ScalewayBackendError>,
{
    let stub = StubApi::start(replies).await?;
    backend.instance_api_base.clone_from(&stub.base_url);
    backend.block_api_base.clone_from(&stub.block_base_url);
    let result = call(&backend).await;
    Ok((result, stub.finish().await?))
}

/// Returns the single captured request, failing when a call was skipped.
///
/// For example, a script expecting one DELETE yields that request, while an
/// empty or multi-request capture reports the count instead of hiding it.
pub(super) fn only_request(requests: &[String]) -> Result<&String> {
    match requests {
        [request] => Ok(request),
        other => Err(anyhow::anyhow!("expected one request, got {}", other.len())),
    }
}

/// Parses a captured request target so tests can inspect its path and query.
///
/// For example, `GET /instance/v1/zones/zone/servers?id=a HTTP/1.1` yields a
/// URL whose path is `/instance/v1/zones/zone/servers`.
pub(super) fn request_url(request: &str) -> Result<reqwest::Url> {
    let path = request
        .split_whitespace()
        .nth(1)
        .context("request path missing")?;
    reqwest::Url::parse(&format!("http://localhost{path}")).context("invalid request URL")
}

/// Returns the body of a captured request, which follows the blank line that
/// ends its headers.
///
/// For example, a request carrying `{"volumes":{}}` yields that value, while a
/// bodyless request reports the missing body instead of an empty object.
pub(super) fn request_body(request: &str) -> Result<serde_json::Value> {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .context("request body missing")?;
    serde_json::from_str(body).context("request body is not JSON")
}

/// Reads one complete HTTP request, rejecting premature close and oversized
/// input.
async fn read_request(stream: &mut TcpStream) -> Result<String> {
    let mut buffer = Vec::new();
    loop {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            bail!("connection closed before request completed");
        }
        buffer.extend(chunk.iter().take(count));
        if buffer.len() > 64 * 1024 {
            bail!("HTTP request exceeded fixture limit");
        }
        let request = std::str::from_utf8(&buffer)?;
        if let Some((headers, body)) = request.split_once("\r\n\r\n") {
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
            if body.len() >= length {
                return Ok(request.to_owned());
            }
        }
    }
}
