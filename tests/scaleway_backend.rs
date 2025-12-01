//! Behavioural tests for the Scaleway backend lifecycle.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::LazyLock;

use mriya::{
    Backend, InstanceHandle, InstanceNetworking, InstanceRequest, ScalewayBackend,
    ScalewayBackendError, ScalewayConfig,
};
use rstest::fixture;
use rstest_bdd::skip;
use rstest_bdd_macros::{given, scenario, then, when};
use tokio::runtime::Runtime;

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    Runtime::new()
        .unwrap_or_else(|err| panic!("tokio runtime should start for behavioural tests: {err}"))
});

fn block_on<Fut, T>(future: Fut) -> Result<T, ScalewayBackendError>
where
    Fut: std::future::Future<Output = Result<T, ScalewayBackendError>>,
{
    RUNTIME.block_on(future)
}

#[fixture]
fn scaleway_config() -> ScalewayConfig {
    match ScalewayConfig::load_from_sources() {
        Ok(config) => config,
        Err(err) => skip!(format!(
            "Skipping Scaleway behavioural tests: failed to load configuration: {err}"
        )),
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

fn provision_and_cleanup(
    backend: &ScalewayBackend,
    request: &InstanceRequest,
) -> Result<InstanceNetworking, ScalewayBackendError> {
    block_on(async {
        let handle: InstanceHandle = match backend.create(request).await {
            Ok(handle) => handle,
            Err(err) if err.to_string().contains("permissions_denied") => {
                skip!("permissions denied during create: {err}")
            }
            Err(err) => return Err(err),
        };
        let ready_result = backend.wait_for_ready(&handle).await;
        let teardown_result = backend.destroy(handle.clone()).await;

        match (ready_result, teardown_result) {
            (Ok(networking), Ok(())) => Ok(networking),
            (Err(wait_err), Ok(())) => Err(wait_err),
            (Ok(_), Err(destroy_err)) => Err(destroy_err),
            (Err(wait_err), Err(destroy_err)) => Err(ScalewayBackendError::Provider {
                message: format!(
                    "wait_for_ready failed with '{wait_err}' before destroy failed with '{destroy_err}'"
                ),
            }),
        }
    })
}

#[given("valid Scaleway credentials")]
fn valid_scaleway_credentials(scaleway_backend: &ScalewayBackend, base_request: &InstanceRequest) {
    let missing = [
        ("SCW_SECRET_KEY", std::env::var("SCW_SECRET_KEY")),
        (
            "SCW_DEFAULT_PROJECT_ID",
            std::env::var("SCW_DEFAULT_PROJECT_ID"),
        ),
    ]
    .into_iter()
    .filter_map(|(name, res)| match res {
        Ok(value) if !value.trim().is_empty() => None,
        _ => Some(name),
    })
    .collect::<Vec<_>>();

    if !missing.is_empty() {
        skip!(format!("missing required Scaleway env: {:?}", missing));
    }

    let _ = (scaleway_backend, base_request);
}

#[when("I provision and tear down a DEV1-S instance from \"{image}\"")]
fn provision_and_teardown(
    scaleway_backend: &ScalewayBackend,
    base_request: InstanceRequest,
    image: String,
) -> Result<InstanceNetworking, ScalewayBackendError> {
    let mut request = base_request;
    request.image_label = image;
    provision_and_cleanup(scaleway_backend, &request)
}

#[then("the backend reports a reachable public IPv4 address")]
fn backend_reports_public_ip(networking: &InstanceNetworking) {
    assert!(matches!(networking.public_ip, IpAddr::V4(_)));
    assert!(networking.public_ip != IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    assert_eq!(networking.ssh_port, 22);
}

#[when("I request instance type \"{instance_type}\"")]
fn request_invalid_type(
    scaleway_backend: &ScalewayBackend,
    base_request: InstanceRequest,
    instance_type: String,
) -> Result<(), ScalewayBackendError> {
    let mut request = base_request;
    request.instance_type = instance_type;
    block_on(async {
        match scaleway_backend.create(&request).await {
            Ok(handle) => {
                scaleway_backend.destroy(handle).await?;
                Err(ScalewayBackendError::Provider {
                    message: String::from("unexpected success"),
                })
            }
            Err(err) if err.to_string().contains("permissions_denied") => {
                skip!("permissions denied during instance creation: {err}")
            }
            Err(ScalewayBackendError::InstanceTypeUnavailable { .. }) => Ok(()),
            Err(ScalewayBackendError::Provider { message })
                if message.contains("commercial_type") || message.contains("invalid_arguments") =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
    })
}

#[then("the request is rejected because the instance type is unavailable")]
#[expect(
    clippy::missing_const_for_fn,
    reason = "step may gather additional assertions"
)]
fn rejects_unknown_type() {}

#[when("I request image label \"{label}\"")]
fn request_invalid_image(
    scaleway_backend: &ScalewayBackend,
    base_request: InstanceRequest,
    label: String,
) -> Result<(), ScalewayBackendError> {
    let mut request = base_request;
    request.image_label = label;
    block_on(async {
        match scaleway_backend.create(&request).await {
            Ok(handle) => {
                scaleway_backend.destroy(handle).await?;
                Err(ScalewayBackendError::Provider {
                    message: String::from("unexpected success"),
                })
            }
            Err(err) if err.to_string().contains("permissions_denied") => {
                skip!("permissions denied during instance creation: {err}")
            }
            Err(ScalewayBackendError::ImageNotFound { .. }) => Ok(()),
            Err(err) => Err(err),
        }
    })
}

#[then("the request is rejected because the image cannot be resolved")]
#[expect(
    clippy::missing_const_for_fn,
    reason = "step may gather additional assertions"
)]
fn rejects_unknown_image() {}

#[scenario(
    path = "tests/features/scaleway_backend.feature",
    name = "Provision and destroy minimal instance"
)]
fn scenario_provision_and_destroy(
    scaleway_backend: ScalewayBackend,
    base_request: InstanceRequest,
) {
    let _ = (scaleway_backend, base_request);
}

#[scenario(
    path = "tests/features/scaleway_backend.feature",
    name = "Reject unknown instance type"
)]
fn scenario_reject_unknown_type(scaleway_backend: ScalewayBackend, base_request: InstanceRequest) {
    let _ = (scaleway_backend, base_request);
}

#[scenario(
    path = "tests/features/scaleway_backend.feature",
    name = "Reject unknown image label"
)]
fn scenario_reject_unknown_image(scaleway_backend: ScalewayBackend, base_request: InstanceRequest) {
    let _ = (scaleway_backend, base_request);
}
