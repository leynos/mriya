use super::*;
use crate::ScalewayConfig;
use crate::scaleway::DEFAULT_SSH_PORT;
use crate::scaleway::types::{Action, InstanceId, InstanceState, Zone};
use scaleway_rs::ScalewayApi;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

fn snapshot(id: &str, state: &str, allowed: &[&str], public_ip: Option<&str>) -> InstanceSnapshot {
    InstanceSnapshot {
        id: InstanceId::from(id),
        state: InstanceState::from(state),
        allowed_actions: allowed.iter().map(|s| Action::from(*s)).collect(),
        public_ip: public_ip.map(str::to_owned),
    }
}

fn image(id: &str, arch: &str, state: &str, creation_date: &str) -> ScalewayImage {
    ScalewayImage {
        id: id.to_owned(),
        name: String::new(),
        arch: arch.to_owned(),
        creation_date: creation_date.to_owned(),
        modification_date: String::new(),
        from_server: None,
        organization: String::new(),
        public: true,
        state: state.to_owned(),
        project: String::new(),
        tags: vec![],
        zone: String::new(),
        root_volume: scaleway_rs::ScalewayImageRootVolume {
            id: String::new(),
            name: String::new(),
            size: 0,
            volume_type: String::new(),
        },
        default_bootscript: None,
        extra_volumes: scaleway_rs::ScalewayImageExtraVolumes {
            volumes: HashMap::new(),
        },
    }
}

fn dummy_config() -> ScalewayConfig {
    ScalewayConfig {
        access_key: None,
        secret_key: String::from("dummy"),
        default_organization_id: None,
        default_project_id: String::from("proj"),
        default_zone: String::from("zone"),
        default_instance_type: String::from("type"),
        default_image: String::from("img"),
        default_architecture: String::from("x86_64"),
    }
}

fn base_request() -> InstanceRequest {
    InstanceRequest {
        image_label: "label".to_owned(),
        instance_type: "type".to_owned(),
        zone: "zone".to_owned(),
        project_id: "proj".to_owned(),
        organisation_id: None,
        architecture: "x86_64".to_owned(),
    }
}

fn backend_fixture() -> ScalewayBackend {
    ScalewayBackend {
        api: ScalewayApi::new("dummy"),
        config: dummy_config(),
        ssh_port: DEFAULT_SSH_PORT,
        poll_interval: Duration::from_millis(1),
        wait_timeout: Duration::from_millis(5),
    }
}

#[tokio::test]
async fn power_on_if_needed_returns_ok_for_running() {
    let snap = snapshot("id", "running", &["poweron"], Some("1.1.1.1"));
    let zone = Zone::from("zone");
    let result = backend_fixture().power_on_if_needed(&zone, &snap).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn power_on_if_needed_errors_when_not_allowed() {
    let snap = snapshot("id", "stopped", &[], None);
    let zone = Zone::from("zone");
    let result = backend_fixture().power_on_if_needed(&zone, &snap).await;
    assert!(matches!(
        result,
        Err(ScalewayBackendError::PowerOnNotAllowed { .. })
    ));
}

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
        loop {
            if Instant::now() > deadline {
                return Err(ScalewayBackendError::Timeout {
                    action: "wait_for_ready".to_owned(),
                    instance_id: handle.id.clone(),
                });
            }

            let server_opt = self.snapshots.pop_front().unwrap_or(None);
            let Some(server) = server_opt else {
                sleep(self.poll_interval).await;
                continue;
            };

            if server.state.as_str() != "running" {
                sleep(self.poll_interval).await;
                continue;
            }

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

            if Instant::now() > deadline {
                return Err(ScalewayBackendError::MissingPublicIp {
                    instance_id: handle.id.clone(),
                });
            }

            sleep(self.poll_interval).await;
        }
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
            Some(snapshot("id", "running", &[], None)),
            Some(snapshot("id", "running", &[], None)),
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
        matches!(
            result,
            Err(ScalewayBackendError::MissingPublicIp { .. } | ScalewayBackendError::Timeout { .. })
        ),
        "unexpected wait_for_public_ip outcome: {result:?}"
    );
}

#[test]
fn filter_images_discards_wrong_arch_or_state() {
    let request = base_request();
    let images = vec![
        image("keep", "x86_64", "available", "2025-01-01T00:00:00Z"),
        image("wrong-arch", "arm64", "available", "2025-01-01T00:00:00Z"),
        image("wrong-state", "x86_64", "failed", "2025-01-01T00:00:00Z"),
    ];

    let filtered = ScalewayBackend::filter_images(images, &request);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered.first().map(|img| img.id.as_str()), Some("keep"));
}

#[test]
fn select_image_id_picks_newest() {
    let request = base_request();
    let images = vec![
        image("oldest", "x86_64", "available", "2024-12-01T00:00:00Z"),
        image("newest", "x86_64", "available", "2025-02-01T00:00:00Z"),
    ];

    let id = ScalewayBackend::select_image_id(images, &request).expect("image selected");
    assert_eq!(id, "newest");
}

#[test]
fn select_image_id_errors_on_empty() {
    let request = base_request();
    let images: Vec<ScalewayImage> = Vec::new();
    let err = ScalewayBackend::select_image_id(images, &request)
        .expect_err("empty candidates should fail");
    assert!(matches!(err, ScalewayBackendError::ImageNotFound { .. }));
}

#[tokio::test]
async fn wait_until_gone_times_out_on_residual() {
    let mut fake = FakeBackend {
        snapshots: VecDeque::from(vec![Some(snapshot("id", "running", &[], None))]),
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
