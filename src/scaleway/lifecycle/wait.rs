//! Readiness and teardown wait helpers for the Scaleway backend.

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
        let deadline = Instant::now() + self.wait_timeout;
        let mut saw_running = false;

        while Instant::now() <= deadline {
            let Some(server) = self.fetch_instance(handle).await? else {
                sleep(self.poll_interval).await;
                continue;
            };

            if server.state.as_str() != "running" {
                sleep(self.poll_interval).await;
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
                    ssh_port: self.ssh_port,
                });
            }

            sleep(self.poll_interval).await;
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
        let deadline = Instant::now() + self.wait_timeout;
        while Instant::now() <= deadline {
            if self.fetch_instance(handle).await?.is_none() {
                return Ok(());
            }
            sleep(self.poll_interval).await;
        }

        Err(ScalewayBackendError::ResidualResource {
            instance_id: handle.id.clone(),
        })
    }
}
