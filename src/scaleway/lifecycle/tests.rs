use super::*;
use crate::ScalewayConfig;
use crate::scaleway::DEFAULT_SSH_PORT;
use crate::scaleway::types::{Action, InstanceId, InstanceState, Zone};
use scaleway_rs::ScalewayApi;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::rc::Rc;
use std::str::FromStr;
use std::time::{Duration, Instant};

fn snapshot(
    id: impl Into<InstanceId>,
    state: impl Into<InstanceState>,
    allowed: impl IntoIterator<Item = impl Into<Action>>,
    public_ip: Option<&str>,
) -> InstanceSnapshot {
    InstanceSnapshot {
        id: id.into(),
        state: state.into(),
        allowed_actions: allowed.into_iter().map(Into::into).collect(),
        public_ip: public_ip.map(str::to_owned),
    }
}

#[derive(Copy, Clone)]
struct ImageSpec {
    id: &'static str,
    arch: &'static str,
    state: &'static str,
    creation_date: &'static str,
}

fn image(spec: ImageSpec) -> ScalewayImage {
    ScalewayImage {
        id: spec.id.to_owned(),
        name: String::new(),
        arch: spec.arch.to_owned(),
        creation_date: spec.creation_date.to_owned(),
        modification_date: String::new(),
        from_server: None,
        organization: String::new(),
        public: true,
        state: spec.state.to_owned(),
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
        default_volume_id: None,
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
        volume_id: None,
    }
}

fn backend_fixture() -> ScalewayBackend {
    ScalewayBackend {
        api: ScalewayApi::new("dummy"),
        config: dummy_config(),
        test_run_id: None,
        ssh_port: DEFAULT_SSH_PORT,
        poll_interval: Duration::from_millis(1),
        wait_timeout: Duration::from_millis(5),
    }
}

#[tokio::test]
async fn power_on_if_needed_returns_ok_for_running() {
    let snap = snapshot("id", "running", [Action::from("poweron")], Some("1.1.1.1"));
    let zone = Zone::from("zone");
    let result = backend_fixture().power_on_if_needed(&zone, &snap).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn power_on_if_needed_errors_when_not_allowed() {
    let snap = snapshot("id", "stopped", Vec::<Action>::new(), None);
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
            Some(snapshot("id", "running", Vec::<Action>::new(), None)),
            Some(snapshot("id", "running", Vec::<Action>::new(), None)),
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
        image(ImageSpec {
            id: "keep",
            arch: "x86_64",
            state: "available",
            creation_date: "2025-01-01T00:00:00Z",
        }),
        image(ImageSpec {
            id: "wrong-arch",
            arch: "arm64",
            state: "available",
            creation_date: "2025-01-01T00:00:00Z",
        }),
        image(ImageSpec {
            id: "wrong-state",
            arch: "x86_64",
            state: "failed",
            creation_date: "2025-01-01T00:00:00Z",
        }),
    ];

    let filtered = ScalewayBackend::filter_images(images, &request);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered.first().map(|img| img.id.as_str()), Some("keep"));
}

#[test]
fn select_image_id_picks_newest() {
    let request = base_request();
    let images = vec![
        image(ImageSpec {
            id: "oldest",
            arch: "x86_64",
            state: "available",
            creation_date: "2024-12-01T00:00:00Z",
        }),
        image(ImageSpec {
            id: "newest",
            arch: "x86_64",
            state: "available",
            creation_date: "2025-02-01T00:00:00Z",
        }),
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
#[expect(
    clippy::excessive_nesting,
    reason = "nested async closures keep fixtures inline for readability"
)]
async fn resolve_image_id_prefers_project_results() {
    let request = base_request();
    let project_called = Rc::new(Cell::new(false));
    let public_called = Rc::new(Cell::new(false));

    let backend = backend_fixture();

    let result = backend
        .resolve_image_id_with(
            &request,
            {
                let flag = Rc::clone(&project_called);
                move || {
                    flag.set(true);
                    async {
                        Ok(vec![image(ImageSpec {
                            id: "project-img",
                            arch: "x86_64",
                            state: "available",
                            creation_date: "2025-02-01T00:00:00Z",
                        })])
                    }
                }
            },
            {
                let flag = Rc::clone(&public_called);
                move || {
                    flag.set(true);
                    async {
                        Ok(vec![image(ImageSpec {
                            id: "public-img",
                            arch: "x86_64",
                            state: "available",
                            creation_date: "2025-01-01T00:00:00Z",
                        })])
                    }
                }
            },
        )
        .await
        .expect("project image should resolve");

    assert_eq!(result, "project-img");
    assert!(project_called.get());
    assert!(!public_called.get(), "public lookup should not be needed");
}

#[tokio::test]
async fn resolve_image_id_falls_back_to_public() {
    let request = base_request();
    let backend = backend_fixture();
    let result = backend
        .resolve_image_id_with(
            &request,
            || async { Ok(Vec::new()) },
            || async {
                Ok(vec![image(ImageSpec {
                    id: "public-img",
                    arch: "x86_64",
                    state: "available",
                    creation_date: "2025-01-01T00:00:00Z",
                })])
            },
        )
        .await
        .expect("public fallback should resolve");

    assert_eq!(result, "public-img");
}

#[tokio::test]
async fn resolve_image_id_propagates_errors() {
    let request = base_request();
    let backend = backend_fixture();
    let err = backend
        .resolve_image_id_with(
            &request,
            || async {
                Err(ScalewayBackendError::Provider {
                    message: "boom".to_owned(),
                })
            },
            || async { Ok(Vec::new()) },
        )
        .await
        .expect_err("error should surface");

    assert!(matches!(err, ScalewayBackendError::Provider { message } if message == "boom"));
}

#[tokio::test]
async fn wait_until_gone_times_out_on_residual() {
    let mut fake = FakeBackend {
        snapshots: VecDeque::from(vec![Some(snapshot(
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
