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
        poll_for_public_ip(
            handle,
            self.ssh_port,
            self.poll_interval,
            self.wait_timeout,
            || self.fetch_instance(handle),
        )
        .await
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

/// Polls `fetch` until the instance reports a running state with a parseable
/// public IP, or the timeout elapses.
///
/// Returns [`ScalewayBackendError::MissingPublicIp`] when a running instance
/// never exposed an address, and [`ScalewayBackendError::Timeout`] when the
/// instance never reached the running state.
pub(super) async fn poll_for_public_ip<F, Fut>(
    handle: &InstanceHandle,
    ssh_port: u16,
    poll_interval: Duration,
    wait_timeout: Duration,
    mut fetch: F,
) -> Result<InstanceNetworking, ScalewayBackendError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<InstanceSnapshot>, ScalewayBackendError>>,
{
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
