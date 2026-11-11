//! Integration tests for passing cloud-init user-data through to Scaleway.
//!
//! This suite provisions a real instance, applies a cloud-init cloud-config
//! payload that installs a package, and verifies the package is available
//! before the remote command executes.

use std::sync::LazyLock;

use mockable::{DefaultEnv, Env};
use mriya::{
    RunOrchestrator,
    ScalewayBackend,
    ScalewayBackendError,
    ScalewayConfig,
    sync::{ProcessCommandRunner, SyncConfig, Syncer},
};
use rstest::fixture;
use rstest_bdd::skip;
use rstest_bdd_macros::{given, scenario, then, when};
use tempfile::TempDir;
use tokio::runtime::Runtime;

static RUNTIME: LazyLock<Result<Runtime, std::io::Error>> = LazyLock::new(Runtime::new);

fn scaleway_integration_enabled() -> bool {
    let enabled = DefaultEnv
        .raw("MRIYA_RUN_SCALEWAY_TESTS")
        .unwrap_or_default();
    matches!(
        enabled.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

/// A live-test dependency that is either ready or unavailable for a reason.
///
/// rstest-bdd 0.6 unwraps `Result` fixtures in the scenario prelude, so an
/// `Err` would fail the scenario before any step could call `skip!`. Live
/// dependencies that are merely switched off therefore travel in this wrapper
/// and the steps decide whether to skip.
#[derive(Clone)]
enum Gated<T> {
    Ready(T),
    Unavailable(String),
}

impl<T> Gated<T> {
    /// Returns the dependency, or skips the running scenario with the reason.
    fn ready_or_skip(self) -> T {
        match self {
            Self::Ready(value) => value,
            Self::Unavailable(reason) => skip!("{}", reason),
        }
    }
}

impl<T> From<Result<T, String>> for Gated<T> {
    fn from(result: Result<T, String>) -> Self {
        result.map_or_else(Self::Unavailable, Self::Ready)
    }
}

#[mriya_test_macros::allow_fixture_expansion_lints]
#[fixture]
fn scaleway_backend() -> Gated<ScalewayBackend> { Gated::from(build_backend()) }

fn build_backend() -> Result<ScalewayBackend, String> {
    if !scaleway_integration_enabled() {
        return Err(String::from(
            "Scaleway integration tests disabled (set MRIYA_RUN_SCALEWAY_TESTS=1 to enable)",
        ));
    }

    let secret = DefaultEnv
        .raw("SCW_SECRET_KEY")
        .map_err(|err| format!("missing SCW_SECRET_KEY: {err}"))?;
    let project = DefaultEnv
        .raw("SCW_DEFAULT_PROJECT_ID")
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

#[mriya_test_macros::allow_fixture_expansion_lints]
#[fixture]
fn syncer() -> Gated<Syncer<ProcessCommandRunner>> { Gated::from(build_syncer()) }

fn build_syncer() -> Result<Syncer<ProcessCommandRunner>, String> {
    if !scaleway_integration_enabled() {
        return Err(String::from(
            "Scaleway integration tests disabled (set MRIYA_RUN_SCALEWAY_TESTS=1 to enable)",
        ));
    }

    let identity = DefaultEnv
        .raw("MRIYA_SYNC_SSH_IDENTITY_FILE")
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
fn temp_source_dir() -> Result<std::sync::Arc<TempDir>, String> {
    TempDir::new()
        .map(std::sync::Arc::new)
        .map_err(|err| format!("temp dir should be created: {err}"))
}

const CLOUD_INIT_JQ: &str = concat!(
    "#cloud-config\n",
    "package_update: true\n",
    "packages:\n",
    "  - jq\n",
);

#[given("valid Scaleway credentials and SSH sync configuration")]
fn valid_scaleway_credentials_and_sync(
    scaleway_backend: Gated<ScalewayBackend>,
    syncer: Gated<Syncer<ProcessCommandRunner>>,
) {
    let _backend = scaleway_backend.ready_or_skip();
    let _syncer = syncer.ready_or_skip();
}

#[when("I provision an instance with cloud-init installing jq and run \"{command}\"")]
fn provision_with_cloud_init_and_run(
    scaleway_backend: Gated<ScalewayBackend>,
    syncer: Gated<Syncer<ProcessCommandRunner>>,
    temp_source_dir: std::sync::Arc<TempDir>,
    command: String,
) -> Result<mriya::sync::RemoteCommandOutput, mriya::RunError<ScalewayBackendError>> {
    let backend = scaleway_backend.ready_or_skip();
    let sync_pipeline = syncer.ready_or_skip();

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

    let runtime = RUNTIME.as_ref().map_err(|err| {
        mriya::RunError::Provision(ScalewayBackendError::Provider {
            message: format!("tokio runtime should start for integration tests: {err}"),
        })
    })?;
    runtime.block_on(async {
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
    _scaleway_backend: Gated<ScalewayBackend>,
    _syncer: Gated<Syncer<ProcessCommandRunner>>,
    _temp_source_dir: Result<std::sync::Arc<TempDir>, String>,
) -> Result<(), String> {
    // The macro unwraps `_temp_source_dir` with `?` before any step runs, so a
    // failure to create the directory fails the scenario rather than being
    // discarded. The steps do the real work.
    Ok(())
}
