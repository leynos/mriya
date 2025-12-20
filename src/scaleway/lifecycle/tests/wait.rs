//! Tests for Scaleway readiness and teardown wait loops.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use scaleway_rs::ScalewayApi;
use tokio::time::sleep;

use crate::backend::{InstanceHandle, InstanceNetworking};
use crate::scaleway::DEFAULT_SSH_PORT;
use crate::scaleway::types::Action;
use crate::scaleway::{ScalewayBackend, ScalewayBackendError};

use super::InstanceSnapshot;

/// Minimal backend double to test wait loops without real API calls.
struct FakeBackend {
    snapshots: VecDeque<Option<InstanceSnapshot>>,
    poll_interval: Duration,
    wait_timeout: Duration,
    ssh_port: u16,
}

impl FakeBackend {
    async fn wait_for_public_ip(
        &mut self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        let mut saw_running = false;

        while Instant::now() <= deadline {
            let server_opt = self.snapshots.pop_front().unwrap_or(None);
            let Some(server) = server_opt else {
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

    async fn wait_until_gone(
        &mut self,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        while Instant::now() <= deadline {
            let next = self.snapshots.pop_front().unwrap_or(None);
            if next.is_none() {
                return Ok(());
            }
            sleep(self.poll_interval).await;
        }
        Err(ScalewayBackendError::ResidualResource {
            instance_id: handle.id.clone(),
        })
    }
}

#[tokio::test]
async fn wait_for_public_ip_returns_missing_ip() {
    let mut fake = FakeBackend {
        snapshots: VecDeque::from(vec![
            Some(super::snapshot("id", "running", Vec::<Action>::new(), None)),
            Some(super::snapshot("id", "running", Vec::<Action>::new(), None)),
        ]),
        poll_interval: Duration::from_millis(1),
        wait_timeout: Duration::from_millis(5),
        ssh_port: DEFAULT_SSH_PORT,
    };
    let handle = InstanceHandle {
        id: "id".to_owned(),
        zone: "zone".to_owned(),
    };
    let result = fake.wait_for_public_ip(&handle).await;
    assert!(
        matches!(result, Err(ScalewayBackendError::MissingPublicIp { .. })),
        "unexpected wait_for_public_ip outcome: {result:?}"
    );
}

#[tokio::test]
async fn wait_for_ssh_ready_succeeds_when_port_listens() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|err| panic!("bind listener: {err}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|err| panic!("listener addr: {err}"));
    tokio::spawn(async move { if let Ok((_stream, _addr)) = listener.accept().await {} });

    let backend = ScalewayBackend {
        api: ScalewayApi::new("dummy"),
        config: super::dummy_config(),
        test_run_id: None,
        ssh_port: DEFAULT_SSH_PORT,
        poll_interval: Duration::from_millis(1),
        wait_timeout: Duration::from_millis(200),
    };

    let handle = InstanceHandle {
        id: String::from("id"),
        zone: String::from("zone"),
    };
    let networking = InstanceNetworking {
        public_ip: addr.ip(),
        ssh_port: addr.port(),
    };
    backend
        .wait_for_ssh_ready(&handle, &networking)
        .await
        .unwrap_or_else(|err| panic!("ssh should be reachable: {err}"));
}

#[tokio::test]
async fn wait_for_ssh_ready_times_out_when_port_closed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|err| panic!("bind listener: {err}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|err| panic!("listener addr: {err}"));
    drop(listener);

    let backend = ScalewayBackend {
        api: ScalewayApi::new("dummy"),
        config: super::dummy_config(),
        test_run_id: None,
        ssh_port: DEFAULT_SSH_PORT,
        poll_interval: Duration::from_millis(1),
        wait_timeout: Duration::from_millis(50),
    };

    let handle = InstanceHandle {
        id: String::from("id"),
        zone: String::from("zone"),
    };
    let networking = InstanceNetworking {
        public_ip: IpAddr::from_str("127.0.0.1")
            .unwrap_or_else(|err| panic!("loopback ip parse: {err}")),
        ssh_port: addr.port(),
    };
    let err = backend
        .wait_for_ssh_ready(&handle, &networking)
        .await
        .expect_err("expected timeout");
    assert!(matches!(err, ScalewayBackendError::Timeout { .. }));
}

#[tokio::test]
async fn wait_until_gone_times_out_on_residual() {
    let mut fake = FakeBackend {
        snapshots: VecDeque::from(vec![Some(super::snapshot(
            "id",
            "running",
            Vec::<Action>::new(),
            None,
        ))]),
        poll_interval: Duration::from_millis(1),
        wait_timeout: Duration::from_millis(2),
        ssh_port: DEFAULT_SSH_PORT,
    };
    let handle = InstanceHandle {
        id: "id".to_owned(),
        zone: "zone".to_owned(),
    };
    let result = fake.wait_until_gone(&handle).await;
    assert!(matches!(
        result,
        Err(ScalewayBackendError::ResidualResource { .. })
    ));
}
