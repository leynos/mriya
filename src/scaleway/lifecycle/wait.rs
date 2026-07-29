//! Readiness and teardown wait helpers for the Scaleway backend.
//!
//! The polling loops are generic over an instance-fetch closure so unit
//! tests can drive the production loop bodies with scripted snapshots
//! instead of duplicating the algorithm against a fake backend.

use std::future::Future;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

use crate::backend::{InstanceHandle, InstanceNetworking};
use crate::scaleway::types::Action;

use super::super::{ScalewayBackend, ScalewayBackendError};
use super::InstanceSnapshot;

const SSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

impl ScalewayBackend {
    pub(in crate::scaleway) async fn fetch_instance(
        &self,
        handle: &InstanceHandle,
    ) -> Result<Option<InstanceSnapshot>, ScalewayBackendError> {
        let mut servers = self
            .api
            .list_instances(&handle.zone)
            .servers(&handle.id)
            .per_page(1)
            .run_async()
            .await?;

        Ok(servers.pop().map(|server| InstanceSnapshot {
            id: server.id.into(),
            state: server.state.into(),
            allowed_actions: server
                .allowed_actions
                .into_iter()
                .map(Action::from)
                .collect(),
            public_ip: server.public_ip.map(|ip| ip.address),
        }))
    }

    pub(in crate::scaleway) async fn wait_for_public_ip(
        &self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, ScalewayBackendError> {
        let settings = PollSettings {
            ssh_port: self.ssh_port,
            poll_interval: self.poll_interval,
            wait_timeout: self.wait_timeout,
        };
        poll_for_public_ip(handle, settings, || self.fetch_instance(handle)).await
    }

    pub(in crate::scaleway) async fn wait_for_ssh_ready(
        &self,
        handle: &InstanceHandle,
        networking: &InstanceNetworking,
    ) -> Result<(), ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        while Instant::now() <= deadline {
            let addr = (networking.public_ip, networking.ssh_port);
            let connect = timeout(SSH_CONNECT_TIMEOUT, TcpStream::connect(addr)).await;
            if matches!(connect, Ok(Ok(_))) {
                return Ok(());
            }
            sleep(self.poll_interval).await;
        }

        Err(ScalewayBackendError::Timeout {
            action: String::from("wait_for_ssh_ready"),
            instance_id: handle.id.clone(),
        })
    }

    pub(in crate::scaleway) async fn wait_until_gone(
        &self,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        poll_until_gone(handle, self.poll_interval, self.wait_timeout, || {
            self.fetch_instance(handle)
        })
        .await
    }
}

/// Timing and port parameters for the readiness polling loop.
#[derive(Clone, Copy)]
pub(super) struct PollSettings {
    pub(super) ssh_port: u16,
    pub(super) poll_interval: Duration,
    pub(super) wait_timeout: Duration,
}

/// Polls `fetch` until the instance reports a running state with a parseable
/// public IP, or the timeout elapses.
///
/// Returns [`ScalewayBackendError::MissingPublicIp`] when a running instance
/// never exposed an address, and [`ScalewayBackendError::Timeout`] when the
/// instance never reached the running state.
///
/// # Examples
///
/// The example is marked `ignore` because the helper is crate-internal and so
/// is unreachable from a doctest; the equivalent assertions run as unit tests
/// in `super::tests::wait`.
///
/// ```ignore
/// let mut script = VecDeque::from(vec![
///     snapshot("id", "starting", [], None),
///     snapshot("id", "running", [], Some("192.0.2.10")),
/// ]);
/// let settings = PollSettings {
///     ssh_port: 22,
///     poll_interval: Duration::from_millis(1),
///     wait_timeout: Duration::from_millis(50),
/// };
/// let networking = poll_for_public_ip(&handle, settings, || {
///     ready(Ok(script.pop_front()))
/// })
/// .await?;
/// assert_eq!(networking.public_ip, IpAddr::from_str("192.0.2.10")?);
/// assert_eq!(networking.ssh_port, 22);
/// ```
pub(super) async fn poll_for_public_ip<F, Fut>(
    handle: &InstanceHandle,
    settings: PollSettings,
    mut fetch: F,
) -> Result<InstanceNetworking, ScalewayBackendError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<InstanceSnapshot>, ScalewayBackendError>>,
{
    let PollSettings {
        ssh_port,
        poll_interval,
        wait_timeout,
    } = settings;
    let deadline = Instant::now() + wait_timeout;
    let mut saw_running = false;

    while Instant::now() <= deadline {
        let Some(server) = fetch().await? else {
            sleep(poll_interval).await;
            continue;
        };

        if server.state.as_str() != "running" {
            sleep(poll_interval).await;
            continue;
        }

        saw_running = true;

        if let Some(address) = server
            .public_ip
            .as_ref()
            .and_then(|ip| IpAddr::from_str(ip).ok())
        {
            return Ok(InstanceNetworking {
                public_ip: address,
                ssh_port,
            });
        }

        sleep(poll_interval).await;
    }

    if saw_running {
        return Err(ScalewayBackendError::MissingPublicIp {
            instance_id: handle.id.clone(),
        });
    }

    Err(ScalewayBackendError::Timeout {
        action: "wait_for_ready".to_owned(),
        instance_id: handle.id.clone(),
    })
}

/// Polls `fetch` until the instance is no longer listed, or the timeout
/// elapses, in which case [`ScalewayBackendError::ResidualResource`] is
/// returned.
///
/// # Examples
///
/// As with [`poll_for_public_ip`], the example is `ignore`d because the helper
/// is crate-internal; the executed equivalents live in `super::tests::wait`.
///
/// ```ignore
/// // The instance is already absent, so the first fetch settles the loop.
/// poll_until_gone(
///     &handle,
///     Duration::from_millis(1),
///     Duration::from_millis(50),
///     || ready(Ok(None)),
/// )
/// .await?;
///
/// // A snapshot that never disappears exhausts the timeout instead.
/// let residual = poll_until_gone(
///     &handle,
///     Duration::from_millis(1),
///     Duration::from_millis(2),
///     || ready(Ok(Some(snapshot("id", "running", [], None)))),
/// )
/// .await;
/// assert!(matches!(
///     residual,
///     Err(ScalewayBackendError::ResidualResource { .. })
/// ));
/// ```
pub(super) async fn poll_until_gone<F, Fut>(
    handle: &InstanceHandle,
    poll_interval: Duration,
    wait_timeout: Duration,
    mut fetch: F,
) -> Result<(), ScalewayBackendError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<InstanceSnapshot>, ScalewayBackendError>>,
{
    let deadline = Instant::now() + wait_timeout;
    while Instant::now() <= deadline {
        if fetch().await?.is_none() {
            return Ok(());
        }
        sleep(poll_interval).await;
    }

    Err(ScalewayBackendError::ResidualResource {
        instance_id: handle.id.clone(),
    })
}
