//! Unit tests for Scaleway lifecycle helpers.

use std::{collections::HashMap, time::Duration};

use rstest::{fixture, rstest};
use scaleway_rs::{ScalewayApi, ScalewayImage};

use super::InstanceSnapshot;
use crate::{
    BootVolumeMismatch,
    ScalewayConfig,
    backend::{InstanceHandle, InstanceRequest},
    boot_volume::{BootVolumePlan, BootVolumeRequest, ImageStorage},
    scaleway::{
        DEFAULT_SSH_PORT,
        ScalewayBackend,
        ScalewayBackendError,
        lifecycle::{
            boot_plan::{ImageFacts, Resolution, resolve_boot_volume},
            products::{
                ConstraintSizes,
                ConstraintsByType,
                ProductCapabilities,
                ProductServerType,
            },
        },
        types::{Action, InstanceId, InstanceState, Zone},
    },
};

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
        // A local root by default: the selection cases are about dates and
        // sources, not storage, and an empty type would now be refused by the
        // storage filter before selection ran.
        root_volume: scaleway_rs::ScalewayImageRootVolume {
            id: String::new(),
            name: String::new(),
            size: 10_000_000_000,
            volume_type: String::from("l_ssd"),
        },
        default_bootscript: None,
        extra_volumes: scaleway_rs::ScalewayImageExtraVolumes {
            volumes: HashMap::new(),
        },
    }
}

#[fixture]
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
        boot_volume_type: None,
        boot_volume_size_gb: None,
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    }
}

#[fixture]
fn base_request() -> InstanceRequest {
    InstanceRequest {
        image_label: "label".to_owned(),
        instance_type: "type".to_owned(),
        zone: "zone".to_owned(),
        project_id: "proj".to_owned(),
        organisation_id: None,
        architecture: "x86_64".to_owned(),
        volume_id: None,
        boot_volume: crate::boot_volume::BootVolumeRequest::default(),
        cloud_init_user_data: None,
    }
}

/// Uses short poll settings and a loopback API base to keep lifecycle tests
/// local.
#[fixture]
fn backend_fixture() -> ScalewayBackend {
    ScalewayBackend {
        api: ScalewayApi::new("dummy"),
        instance_api_base: String::from("http://127.0.0.1:9/instance/v1"),
        block_api_base: String::from("http://127.0.0.1:9/block/v1"),
        config: dummy_config(),
        test_run_id: None,
        ssh_port: DEFAULT_SSH_PORT,
        poll_interval: Duration::from_millis(1),
        wait_timeout: Duration::from_millis(5),
    }
}

#[rstest]
#[tokio::test]
async fn power_on_if_needed_returns_ok_for_running(backend_fixture: ScalewayBackend) {
    let snap = snapshot("id", "running", [Action::from("poweron")], Some("1.1.1.1"));
    let zone = Zone::from("zone");
    let result = backend_fixture.power_on_if_needed(&zone, &snap).await;
    assert!(
        result.is_ok(),
        "expected Ok for running instance snapshot, got {result:?}"
    );
}

// Exact poweron matching kills the equality-mutant survivor tracked in #58.
/// Rejects stopped snapshots unless `poweron` is an allowed action.
#[rstest]
#[case::no_actions(Vec::<Action>::new())]
#[case::unrelated_action(vec![Action::from("reboot")])]
#[tokio::test]
async fn power_on_if_needed_errors_when_not_allowed(
    backend_fixture: ScalewayBackend,
    #[case] allowed: Vec<Action>,
) {
    let snap = snapshot("id", "stopped", allowed, None);
    let result = backend_fixture
        .power_on_if_needed(&Zone::from("zone"), &snap)
        .await;
    assert!(
        matches!(result, Err(ScalewayBackendError::PowerOnNotAllowed { .. })),
        "expected PowerOnNotAllowed error, got {result:?}"
    );
}

/// Supplies the stable instance identity shared by lifecycle test cases.
#[fixture]
fn handle() -> InstanceHandle {
    InstanceHandle {
        id: String::from("id"),
        zone: String::from("zone"),
        boot_volume: None,
    }
}

/// Reports whether `error` is a root-volume mismatch of the `Differs` kind.
fn is_differing_mismatch(error: &ScalewayBackendError) -> bool {
    match error {
        ScalewayBackendError::BootVolumeMismatch(mismatch) => {
            matches!(**mismatch, BootVolumeMismatch::Differs { .. })
        }
        _ => false,
    }
}

/// A 20–40 GB local range, for types that offer local storage.
fn local_range() -> ConstraintSizes {
    ConstraintSizes {
        min_size: 20_000_000_000,
        max_size: 40_000_000_000,
    }
}

/// A general 10–100 GB range, applied to every volume on the type.
fn general_range() -> ConstraintSizes {
    ConstraintSizes {
        min_size: 10_000_000_000,
        max_size: 100_000_000_000,
    }
}

/// Builds a catalogue entry with the chosen storage support.
fn product(
    arch: &str,
    general: ConstraintSizes,
    local: Option<ConstraintSizes>,
    block_storage: Option<bool>,
) -> ProductServerType {
    ProductServerType {
        volumes_constraint: general,
        per_volume_constraint: local.map(|sizes| ConstraintsByType { l_ssd: Some(sizes) }),
        arch: arch.to_owned(),
        end_of_service: false,
        capabilities: ProductCapabilities { block_storage },
    }
}

/// A type offering both local and Block Storage, with the standard ranges.
fn dual_product() -> ProductServerType {
    product("x86_64", general_range(), Some(local_range()), Some(true))
}

/// Image facts for an image whose root is `size_bytes` and reports
/// `volume_type`.
fn root_image(
    size_bytes: u64,
    storage: ImageStorage,
    volume_type: &'static str,
) -> ImageFacts<'static> {
    ImageFacts {
        size_bytes,
        storage,
        volume_type,
    }
}

/// A 10 GB local-only image.
fn local_image() -> ImageFacts<'static> {
    root_image(10_000_000_000, ImageStorage::new(true, false), "l_ssd")
}

/// A 10 GB Block-Storage-only image.
fn sbs_image() -> ImageFacts<'static> {
    root_image(
        10_000_000_000,
        ImageStorage::new(false, true),
        "sbs_snapshot",
    )
}

/// A 10 GB image that can back either storage type.
fn dual_image() -> ImageFacts<'static> {
    root_image(10_000_000_000, ImageStorage::new(true, true), "unified")
}

/// Resolves with the common architecture and a named instance type.
fn resolve(
    request: BootVolumeRequest,
    product: &ProductServerType,
    image: ImageFacts<'_>,
) -> Result<BootVolumePlan, ScalewayBackendError> {
    resolve_boot_volume(Resolution {
        instance_type: "DEV1-S",
        request: &request,
        product,
        image,
        requested_architecture: "x86_64",
    })
}

mod boot_plan;
mod boot_plan_defaults;
mod create_cleanup;
mod image;
mod wait;

mod http_stub;
mod instances;
mod network;
mod products;
mod properties;
mod root_volume;
mod volume_sbs;
mod volumes;
