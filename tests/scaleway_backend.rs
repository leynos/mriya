use std::net::{IpAddr, Ipv4Addr};

use mriya::{InstanceNetworking, InstanceRequest, ScalewayBackend, ScalewayBackendError, ScalewayConfig};
use rstest::fixture;
use rstest_bdd::{given, scenario, then, when};

#[fixture]
fn scaleway_config() -> ScalewayConfig {
    match ScalewayConfig::load_from_sources() {
        Ok(cfg) => cfg,
        Err(err) => panic!("failed to load Scaleway configuration: {err}"),
    }
}

#[fixture]
fn scaleway_backend(scaleway_config: ScalewayConfig) -> ScalewayBackend {
    match ScalewayBackend::new(scaleway_config) {
        Ok(backend) => backend,
        Err(err) => panic!("failed to construct backend: {err}"),
    }
}

#[fixture]
fn base_request(scaleway_config: ScalewayConfig) -> InstanceRequest {
    match scaleway_config.as_request() {
        Ok(request) => request,
        Err(err) => panic!("invalid base request: {err}"),
    }
}

async fn provision_and_cleanup(
    backend: &ScalewayBackend,
    request: InstanceRequest,
) -> Result<InstanceNetworking, ScalewayBackendError> {
    let handle = backend.create(&request).await?;
    let ready_result = backend.wait_for_ready(&handle).await;
    let teardown_result = backend.destroy(handle.clone()).await;

    match (ready_result, teardown_result) {
        (Ok(networking), Ok(())) => Ok(networking),
        (Err(wait_err), Ok(())) => Err(wait_err),
        (Ok(_), Err(destroy_err)) | (Err(_), Err(destroy_err)) => Err(destroy_err),
    }
}

#[given("valid Scaleway credentials")]
fn valid_scaleway_credentials(
    scaleway_backend: ScalewayBackend,
    base_request: InstanceRequest,
) -> (ScalewayBackend, InstanceRequest) {
    (scaleway_backend, base_request)
}

#[when("I provision and tear down a DEV1-S instance from \"{image}\"")]
async fn provision_and_teardown(
    ctx: (ScalewayBackend, InstanceRequest),
    image: String,
) -> Result<InstanceNetworking, ScalewayBackendError> {
    let (backend, mut request) = ctx;
    request.image_label = image;
    provision_and_cleanup(&backend, request).await
}

#[then("the backend reports a reachable public IPv4 address")]
fn backend_reports_public_ip(networking: InstanceNetworking) {
    assert!(matches!(networking.public_ip, IpAddr::V4(_)));
    assert!(networking.public_ip != IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    assert_eq!(networking.ssh_port, 22);
}

#[when("I request instance type \"{instance_type}\"")]
async fn request_invalid_type(
    ctx: (ScalewayBackend, InstanceRequest),
    instance_type: String,
) -> Result<ScalewayBackendError, ScalewayBackendError> {
    let (backend, mut request) = ctx;
    request.instance_type = instance_type;
    match backend.create(&request).await {
        Ok(handle) => {
            let _ = backend.destroy(handle).await;
            Err(ScalewayBackendError::Provider {
                message: String::from("unexpected success"),
            })
        }
        Err(err) => Ok(err),
    }
}

#[then("the request is rejected because the instance type is unavailable")]
fn rejects_unknown_type(err: ScalewayBackendError) {
    assert!(matches!(
        err,
        ScalewayBackendError::InstanceTypeUnavailable { .. }
    ));
}

#[when("I request image label \"{label}\"")]
async fn request_invalid_image(
    ctx: (ScalewayBackend, InstanceRequest),
    label: String,
) -> Result<ScalewayBackendError, ScalewayBackendError> {
    let (backend, mut request) = ctx;
    request.image_label = label;
    match backend.create(&request).await {
        Ok(handle) => {
            let _ = backend.destroy(handle).await;
            Err(ScalewayBackendError::Provider {
                message: String::from("unexpected success"),
            })
        }
        Err(err) => Ok(err),
    }
}

#[then("the request is rejected because the image cannot be resolved")]
fn rejects_unknown_image(err: ScalewayBackendError) {
    assert!(matches!(err, ScalewayBackendError::ImageNotFound { .. }));
}

#[scenario(path = "tests/features/scaleway_backend.feature", name = "Provision and destroy minimal instance")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_provision_and_destroy(
    scaleway_backend: ScalewayBackend,
    base_request: InstanceRequest,
) {
    let _ = (scaleway_backend, base_request);
}

#[scenario(path = "tests/features/scaleway_backend.feature", name = "Reject unknown instance type")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_reject_unknown_type(
    scaleway_backend: ScalewayBackend,
    base_request: InstanceRequest,
) {
    let _ = (scaleway_backend, base_request);
}

#[scenario(path = "tests/features/scaleway_backend.feature", name = "Reject unknown image label")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_reject_unknown_image(
    scaleway_backend: ScalewayBackend,
    base_request: InstanceRequest,
) {
    let _ = (scaleway_backend, base_request);
}
