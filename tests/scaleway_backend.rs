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

fn scaleway_integration_enabled() -> bool {
    let enabled = std::env::var("MRIYA_RUN_SCALEWAY_TESTS").unwrap_or_default();
    matches!(
        enabled.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

#[fixture]
fn scaleway_config() -> Option<ScalewayConfig> {
    if !scaleway_integration_enabled() {
        return None;
    }

    let secret = std::env::var("SCW_SECRET_KEY").ok()?;
    let project = std::env::var("SCW_DEFAULT_PROJECT_ID").ok()?;
    if secret.trim().is_empty() || project.trim().is_empty() {
        return None;
    }
    ScalewayConfig::load_from_sources().ok()
}

#[fixture]
fn scaleway_backend(scaleway_config: Option<ScalewayConfig>) -> Option<ScalewayBackend> {
    scaleway_config.and_then(|cfg| ScalewayBackend::new(cfg).ok())
}

#[fixture]
fn base_request(scaleway_config: Option<ScalewayConfig>) -> Option<InstanceRequest> {
    scaleway_config.and_then(|cfg| cfg.as_request().ok())
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

/// Helper function to test backend creation with invalid requests.
///
/// Applies `modify_request` to a clone of `base_request`, attempts to create
/// an instance, and verifies the error matches expectations via
/// `is_expected_error`.
fn test_invalid_request(
    scaleway_backend: &ScalewayBackend,
    base_request: InstanceRequest,
    modify_request: impl FnOnce(&mut InstanceRequest),
    is_expected_error: impl Fn(&ScalewayBackendError) -> bool,
) -> Result<(), ScalewayBackendError> {
    let mut request = base_request;
    modify_request(&mut request);

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
            Err(err) if is_expected_error(&err) => Ok(()),
            Err(err) => Err(err),
        }
    })
}

#[given("valid Scaleway credentials")]
fn valid_scaleway_credentials(
    scaleway_config: Option<ScalewayConfig>,
    scaleway_backend: Option<ScalewayBackend>,
    base_request: Option<InstanceRequest>,
) {
    match (scaleway_config, scaleway_backend, base_request) {
        (Some(_), Some(_), Some(_)) => {}
        _ => skip!("Scaleway credentials not available"),
    }
}

#[when("I provision and tear down an instance from \"{image}\"")]
fn provision_and_teardown(
    scaleway_backend: Option<ScalewayBackend>,
    base_request: Option<InstanceRequest>,
    image: String,
) -> Result<InstanceNetworking, ScalewayBackendError> {
    let backend_owned =
        scaleway_backend.unwrap_or_else(|| skip!("Scaleway backend fixture unavailable"));
    let backend = &backend_owned;
    let mut request =
        base_request.unwrap_or_else(|| skip!("Scaleway base request fixture unavailable"));
    request.image_label = image;
    provision_and_cleanup(backend, &request)
}

#[then("the backend reports a reachable public IPv4 address")]
fn backend_reports_public_ip(networking: &InstanceNetworking) {
    assert!(matches!(networking.public_ip, IpAddr::V4(_)));
    assert!(networking.public_ip != IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    assert_eq!(networking.ssh_port, 22);
}

#[when("I request instance type \"{instance_type}\"")]
fn request_invalid_type(
    scaleway_backend: Option<ScalewayBackend>,
    base_request: Option<InstanceRequest>,
    instance_type: String,
) -> Result<(), ScalewayBackendError> {
    let backend_owned =
        scaleway_backend.unwrap_or_else(|| skip!("Scaleway backend fixture unavailable"));
    let backend = &backend_owned;
    let request_template =
        base_request.unwrap_or_else(|| skip!("Scaleway base request fixture unavailable"));

    test_invalid_request(
        backend,
        request_template,
        |req| req.instance_type = instance_type,
        |err| {
            matches!(err, ScalewayBackendError::InstanceTypeUnavailable { .. })
                || matches!(
                    err,
                    ScalewayBackendError::Provider { message }
                        if message.contains("commercial_type")
                            || message.contains("invalid_arguments")
                )
        },
    )
}

#[then("the request is rejected because the instance type is unavailable")]
fn rejects_unknown_type() {}

#[when("I request image label \"{label}\"")]
fn request_invalid_image(
    scaleway_backend: Option<ScalewayBackend>,
    base_request: Option<InstanceRequest>,
    label: String,
) -> Result<(), ScalewayBackendError> {
    let backend_owned =
        scaleway_backend.unwrap_or_else(|| skip!("Scaleway backend fixture unavailable"));
    let backend = &backend_owned;
    let request_template =
        base_request.unwrap_or_else(|| skip!("Scaleway base request fixture unavailable"));

    test_invalid_request(
        backend,
        request_template,
        |req| req.image_label = label,
        |err| matches!(err, ScalewayBackendError::ImageNotFound { .. }),
    )
}

#[then("the request is rejected because the image cannot be resolved")]
fn rejects_unknown_image() {}

#[scenario(
    path = "tests/features/scaleway_backend.feature",
    name = "Provision and destroy minimal instance"
)]
fn scenario_provision_and_destroy(
    scaleway_config: Option<ScalewayConfig>,
    scaleway_backend: Option<ScalewayBackend>,
    base_request: Option<InstanceRequest>,
) {
    let _ = (scaleway_config, scaleway_backend, base_request);
}

#[scenario(
    path = "tests/features/scaleway_backend.feature",
    name = "Reject unknown instance type"
)]
fn scenario_reject_unknown_type(
    scaleway_config: Option<ScalewayConfig>,
    scaleway_backend: Option<ScalewayBackend>,
    base_request: Option<InstanceRequest>,
) {
    let _ = (scaleway_config, scaleway_backend, base_request);
}

#[scenario(
    path = "tests/features/scaleway_backend.feature",
    name = "Reject unknown image label"
)]
fn scenario_reject_unknown_image(
    scaleway_config: Option<ScalewayConfig>,
    scaleway_backend: Option<ScalewayBackend>,
    base_request: Option<InstanceRequest>,
) {
    let _ = (scaleway_config, scaleway_backend, base_request);
}
