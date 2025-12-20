//! Integration tests for passing cloud-init user-data through to Scaleway.
//!
//! This suite provisions a real instance, applies a cloud-init cloud-config
//! payload that installs a package, and verifies the package is available
//! before the remote command executes.

use std::sync::LazyLock;

use mriya::sync::{ProcessCommandRunner, SyncConfig, Syncer};
use mriya::{RunOrchestrator, ScalewayBackend, ScalewayBackendError, ScalewayConfig};
use rstest::fixture;
use rstest_bdd::skip;
use rstest_bdd_macros::{given, scenario, then, when};
use tempfile::TempDir;
use tokio::runtime::Runtime;

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    Runtime::new()
        .unwrap_or_else(|err| panic!("tokio runtime should start for integration tests: {err}"))
});

fn scaleway_integration_enabled() -> bool {
    let enabled = std::env::var("MRIYA_RUN_SCALEWAY_TESTS").unwrap_or_default();
    matches!(
        enabled.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

#[fixture]
fn scaleway_backend() -> Result<ScalewayBackend, String> {
    if !scaleway_integration_enabled() {
        return Err(String::from(
            "Scaleway integration tests disabled (set MRIYA_RUN_SCALEWAY_TESTS=1 to enable)",
        ));
    }

    let secret =
        std::env::var("SCW_SECRET_KEY").map_err(|err| format!("missing SCW_SECRET_KEY: {err}"))?;
    let project = std::env::var("SCW_DEFAULT_PROJECT_ID")
        .map_err(|err| format!("missing SCW_DEFAULT_PROJECT_ID: {err}"))?;
    if secret.trim().is_empty() || project.trim().is_empty() {
        return Err(String::from(
            "empty SCW_SECRET_KEY or SCW_DEFAULT_PROJECT_ID",
        ));
    }

    let config = ScalewayConfig::load_without_cli_args()
        .map_err(|err| format!("failed to load config: {err}"))?;
    ScalewayBackend::new(config).map_err(|err| format!("failed to build backend: {err}"))
}

#[fixture]
fn syncer() -> Result<Syncer<ProcessCommandRunner>, String> {
    if !scaleway_integration_enabled() {
        return Err(String::from(
            "Scaleway integration tests disabled (set MRIYA_RUN_SCALEWAY_TESTS=1 to enable)",
        ));
    }

    let identity = std::env::var("MRIYA_SYNC_SSH_IDENTITY_FILE")
        .map_err(|err| format!("missing MRIYA_SYNC_SSH_IDENTITY_FILE: {err}"))?;
    if identity.trim().is_empty() {
        return Err(String::from("empty MRIYA_SYNC_SSH_IDENTITY_FILE"));
    }

    let sync_config = SyncConfig::load_without_cli_args()
        .map_err(|err| format!("failed to load sync config: {err}"))?;
    Syncer::new(sync_config, ProcessCommandRunner)
        .map_err(|err| format!("syncer init failed: {err}"))
}

#[fixture]
fn temp_source_dir() -> std::sync::Arc<TempDir> {
    std::sync::Arc::new(
        TempDir::new().unwrap_or_else(|err| panic!("temp dir should be created: {err}")),
    )
}

const CLOUD_INIT_JQ: &str = concat!(
    "#cloud-config\n",
    "package_update: true\n",
    "packages:\n",
    "  - jq\n",
);

#[given("valid Scaleway credentials and SSH sync configuration")]
fn valid_scaleway_credentials_and_sync(
    scaleway_backend: Result<ScalewayBackend, String>,
    syncer: Result<Syncer<ProcessCommandRunner>, String>,
) {
    let _backend = scaleway_backend.unwrap_or_else(|err| skip!("{}", err));
    let _syncer = syncer.unwrap_or_else(|err| skip!("{}", err));
}

#[when("I provision an instance with cloud-init installing jq and run \"{command}\"")]
fn provision_with_cloud_init_and_run(
    scaleway_backend: Result<ScalewayBackend, String>,
    syncer: Result<Syncer<ProcessCommandRunner>, String>,
    temp_source_dir: std::sync::Arc<TempDir>,
    command: String,
) -> Result<mriya::sync::RemoteCommandOutput, mriya::RunError<ScalewayBackendError>> {
    let backend = scaleway_backend.unwrap_or_else(|err| skip!("{}", err));
    let sync_pipeline = syncer.unwrap_or_else(|err| skip!("{}", err));

    let mut request = backend
        .default_request()
        .map_err(mriya::RunError::Provision)?;
    request.cloud_init_user_data = Some(String::from(CLOUD_INIT_JQ));

    let source = camino::Utf8PathBuf::from_path_buf(temp_source_dir.path().to_path_buf()).map_err(
        |path| {
            mriya::RunError::Provision(ScalewayBackendError::Provider {
                message: format!("temp dir should be utf8: {}", path.display()),
            })
        },
    )?;

    let orchestrator: RunOrchestrator<ScalewayBackend, ProcessCommandRunner> =
        RunOrchestrator::new(backend, sync_pipeline);

    RUNTIME.block_on(async {
        orchestrator
            .execute(&request, &source, command.as_str())
            .await
    })
}

#[then("the remote command succeeds and reports a jq version")]
fn jq_available(output: &mriya::sync::RemoteCommandOutput) {
    assert_eq!(
        output.exit_code,
        Some(0),
        "expected successful exit, got stderr: {}",
        output.stderr
    );
    assert!(
        output.stdout.contains("jq-"),
        "expected stdout to contain jq version, got: {}",
        output.stdout
    );
}

#[scenario(
    path = "tests/features/scaleway_cloud_init.feature",
    name = "Cloud-init installs packages before the command runs"
)]
fn scenario_cloud_init_install_jq(
    scaleway_backend: Result<ScalewayBackend, String>,
    syncer: Result<Syncer<ProcessCommandRunner>, String>,
    temp_source_dir: std::sync::Arc<TempDir>,
) {
    drop((scaleway_backend, syncer, temp_source_dir));
}
