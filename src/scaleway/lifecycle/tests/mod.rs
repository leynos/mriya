//! Unit tests for Scaleway lifecycle helpers.

use std::collections::HashMap;
use std::time::Duration;

use scaleway_rs::{ScalewayApi, ScalewayImage};

use super::InstanceSnapshot;
use crate::ScalewayConfig;
use crate::backend::InstanceRequest;
use crate::scaleway::DEFAULT_SSH_PORT;
use crate::scaleway::types::{Action, InstanceId, InstanceState, Zone};
use crate::scaleway::{ScalewayBackend, ScalewayBackendError};

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
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
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
        cloud_init_user_data: None,
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

mod image;
mod wait;
