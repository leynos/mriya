//! Loopback HTTP scripts for lifecycle boundary tests.
//!
//! Every response closes its connection, and callers await the task with a
//! bounded timeout so skipped delegation cannot leave a test hanging.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub(super) struct StubApi {
    pub(super) base_url: String,
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
    /// complete request text is returned by `finish` for assertions.
    pub(super) async fn start(replies: Vec<Reply>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().await?;
                requests.push(read_request(&mut stream).await?);
                let response = format!(
                    "HTTP/1.1 {} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.status,
                    reply.body.len(),
                    reply.body,
                );
                stream.write_all(response.as_bytes()).await?;
                stream.shutdown().await?;
            }
            Ok(requests)
        });
        Ok(Self {
            base_url: format!("http://{address}/instance/v1"),
            task,
        })
    }

    /// Waits for all exchanges, failing if a production call was skipped.
    ///
    /// For example, a script expecting DELETE followed by GET fails after two
    /// seconds if teardown incorrectly returns before its GET request.
    pub(super) async fn finish(self) -> Result<Vec<String>> {
        tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .context("lifecycle did not consume the HTTP script")?
            .context("HTTP script task failed")?
    }
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
