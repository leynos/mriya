//! Tests for Scaleway readiness and teardown wait loops.
//!
//! The polling loops are exercised directly through the production
//! `poll_for_public_ip` and `poll_until_gone` helpers with scripted fetch
//! closures, so mutations to the loop bodies are observable.

use std::collections::VecDeque;
use std::future::{Ready, ready};
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;

use scaleway_rs::ScalewayApi;

use crate::backend::{InstanceHandle, InstanceNetworking};
use crate::scaleway::DEFAULT_SSH_PORT;
use crate::scaleway::types::Action;
use crate::scaleway::{ScalewayBackend, ScalewayBackendError};

use super::super::wait::{poll_for_public_ip, poll_until_gone};
use super::InstanceSnapshot;

type FetchResult = Result<Option<InstanceSnapshot>, ScalewayBackendError>;

const POLL_INTERVAL: Duration = Duration::from_millis(1);
const WAIT_TIMEOUT: Duration = Duration::from_millis(50);

/// Returns a fetch closure yielding the given snapshots in order, then
/// `None` once the script is exhausted.
fn scripted_fetch(snapshots: Vec<Option<InstanceSnapshot>>) -> impl FnMut() -> Ready<FetchResult> {
    let mut queue = VecDeque::from(snapshots);
    move || ready(Ok(queue.pop_front().unwrap_or(None)))
}

fn handle() -> InstanceHandle {
    InstanceHandle {
        id: "id".to_owned(),
        zone: "zone".to_owned(),
    }
}

#[tokio::test]
async fn wait_for_public_ip_returns_networking_once_running() {
    let fetch = scripted_fetch(vec![
        Some(super::snapshot(
            "id",
            "starting",
            Vec::<Action>::new(),
            None,
        )),
        Some(super::snapshot(
            "id",
            "running",
            Vec::<Action>::new(),
            Some("192.0.2.10"),
        )),
    ]);
    let networking = poll_for_public_ip(
        &handle(),
        DEFAULT_SSH_PORT,
        POLL_INTERVAL,
        WAIT_TIMEOUT,
        fetch,
    )
    .await
    .unwrap_or_else(|err| panic!("expected networking, got {err}"));
    let expected_ip =
        IpAddr::from_str("192.0.2.10").unwrap_or_else(|err| panic!("ip parse: {err}"));
    assert_eq!(networking.public_ip, expected_ip);
    assert_eq!(networking.ssh_port, DEFAULT_SSH_PORT);
}

#[tokio::test]
async fn wait_for_public_ip_returns_missing_ip() {
    let fetch = scripted_fetch(vec![
        Some(super::snapshot("id", "running", Vec::<Action>::new(), None)),
        Some(super::snapshot("id", "running", Vec::<Action>::new(), None)),
    ]);
    let result = poll_for_public_ip(
        &handle(),
        DEFAULT_SSH_PORT,
        POLL_INTERVAL,
        Duration::from_millis(5),
        fetch,
    )
    .await;
    assert!(
        matches!(result, Err(ScalewayBackendError::MissingPublicIp { .. })),
        "unexpected wait_for_public_ip outcome: {result:?}"
    );
}

#[tokio::test]
async fn wait_until_gone_returns_ok_when_instance_absent() {
    let fetch = scripted_fetch(vec![None]);
    let result = poll_until_gone(&handle(), POLL_INTERVAL, WAIT_TIMEOUT, fetch).await;
    assert!(
        result.is_ok(),
        "expected Ok for absent instance: {result:?}"
    );
}

#[tokio::test]
async fn wait_until_gone_times_out_on_residual() {
    let snapshot = super::snapshot("id", "running", Vec::<Action>::new(), None);
    let fetch = move || ready(Ok(Some(snapshot.clone())));
    let result = poll_until_gone(&handle(), POLL_INTERVAL, Duration::from_millis(2), fetch).await;
    assert!(matches!(
        result,
        Err(ScalewayBackendError::ResidualResource { .. })
    ));
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

    let networking = InstanceNetworking {
        public_ip: addr.ip(),
        ssh_port: addr.port(),
    };
    backend
        .wait_for_ssh_ready(&handle(), &networking)
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

    let networking = InstanceNetworking {
        public_ip: IpAddr::from_str("127.0.0.1")
            .unwrap_or_else(|err| panic!("loopback ip parse: {err}")),
        ssh_port: addr.port(),
    };
    let err = backend
        .wait_for_ssh_ready(&handle(), &networking)
        .await
        .expect_err("expected timeout");
    assert!(matches!(err, ScalewayBackendError::Timeout { .. }));
}
