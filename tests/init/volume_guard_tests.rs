//! Connected tests for the volume-ID guard.
//!
//! These drive the real loader, the composition-root request builder, and the
//! mocked-backend orchestrator together, so the blank and case-variant
//! environment provenance is exercised end to end.

use mriya::{InitOrchestrator, sync::Syncer};
use tokio::runtime::Runtime;

use super::bdd_steps::StepError;

/// Loads a real configuration from an injected file and environment, then
/// builds the init request the composition root would use.
fn loaded_request_with_environment(
    key: &str,
    value: &str,
) -> Result<mriya::InitRequest, StepError> {
    use std::sync::Arc;

    use mriya::{InitConfig, ScalewayConfig, request_from_loaded_config};
    use ortho_config::{MapEnv, OrthoConfig as _};

    let tmp = tempfile::TempDir::new().map_err(|err| StepError::Assertion(err.to_string()))?;
    let root = camino::Utf8Path::from_path(tmp.path())
        .ok_or_else(|| StepError::Assertion(String::from("temp path should be UTF-8")))?;
    let dir = cap_std::fs_utf8::Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .map_err(|err| StepError::Assertion(err.to_string()))?;
    dir.write(
        "mriya.toml",
        concat!(
            "secret_key = \"secret\"\n",
            "default_project_id = \"project\"\n",
            "default_volume_id = \"file-volume\"\n"
        ),
    )
    .map_err(|err| StepError::Assertion(err.to_string()))?;
    let file = root.join("mriya.toml");
    let env = MapEnv::new()
        .with_current_dir(root)
        .with_var("MRIYA_CONFIG_PATH", file.as_str())
        .with_var(key, value);
    let shared = Arc::new(env);
    let scaleway =
        ScalewayConfig::load_from_iter_with_sources(["mriya"], shared.clone(), shared.clone())
            .map_err(|err| StepError::Assertion(err.to_string()))?;

    request_from_loaded_config(
        mriya::InitRequestInputs {
            scaleway: &scaleway,
            init_config: &InitConfig { volume_size_gb: 20 },
            project_name: "project",
            overwrite_existing_volume_id: true,
        },
        shared.as_ref(),
    )
    .map_err(|err| StepError::Assertion(err.to_string()))
}

/// The refusal a connected case must observe.
#[derive(Clone, Copy, Debug)]
enum ExpectedRefusal {
    /// A non-blank environment value with this trimmed ID.
    Environment(&'static str),
    /// A present blank value under the canonical variable name.
    Blank,
}

/// The whole composition root over a real load: configuration comes from an
/// injected file and environment, the scan source supplies the volume-ID
/// provenance, and the mocked backend must record no volume creation. Covers
/// a case-variant key and a present but blank value.
#[rstest::rstest]
#[case::case_variant(
    "scw_default_volume_id",
    "old-id",
    ExpectedRefusal::Environment("old-id")
)]
#[case::blank("SCW_DEFAULT_VOLUME_ID", "   ", ExpectedRefusal::Blank)]
fn init_refuses_an_environment_volume_id(
    #[case] key: &str,
    #[case] value: &str,
    #[case] expected: ExpectedRefusal,
) -> Result<(), StepError> {
    use mriya::InitError;

    use super::{
        test_doubles::{MemoryConfigStore, ScriptedVolumeBackend},
        test_helpers::InitTestError,
    };
    use crate::sync_config::sync_config;

    let request = loaded_request_with_environment(key, value)?;
    let backend = ScriptedVolumeBackend::new();
    let runner = mriya::test_support::ScriptedRunner::new();
    let syncer = Syncer::new(sync_config(), runner).map_err(InitTestError::from)?;
    let store = MemoryConfigStore::with_existing("file-volume");
    let orchestrator = InitOrchestrator::new(backend.clone(), syncer, store);
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;

    let result = runtime.block_on(async { orchestrator.execute(&request).await });

    let refused = match (expected, &result) {
        (ExpectedRefusal::Environment(id), Err(InitError::EnvironmentVolumeId { volume_id })) => {
            volume_id == id
        }
        (ExpectedRefusal::Blank, Err(InitError::BlankEnvironmentVolumeId { variable })) => {
            *variable == "SCW_DEFAULT_VOLUME_ID"
        }
        _ => false,
    };
    if !refused {
        return Err(StepError::Assertion(format!(
            "expected {expected:?} refusal, got {result:?}"
        )));
    }
    if backend.create_volume_calls() != 0 {
        return Err(StepError::Assertion(format!(
            "the guard must refuse before creating a volume, calls={}",
            backend.create_volume_calls()
        )));
    }
    Ok(())
}
