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

/// Builds an orchestrator over the scripted backend and an empty store.
fn orchestrator_with_empty_store() -> Result<
    (
        InitOrchestrator<
            super::test_doubles::ScriptedVolumeBackend,
            mriya::test_support::ScriptedRunner,
            super::test_doubles::MemoryConfigStore,
        >,
        super::test_doubles::ScriptedVolumeBackend,
    ),
    StepError,
> {
    use super::{
        test_doubles::{MemoryConfigStore, ScriptedVolumeBackend},
        test_helpers::InitTestError,
    };
    use crate::sync_config::sync_config;

    let backend = ScriptedVolumeBackend::new();
    let runner = mriya::test_support::ScriptedRunner::new();
    let syncer = Syncer::new(sync_config(), runner).map_err(InitTestError::from)?;
    Ok((
        InitOrchestrator::new(backend.clone(), syncer, MemoryConfigStore::new()),
        backend,
    ))
}

/// The public constructor records a file source; without `--force` the
/// orchestrator refuses it even when the store holds no ID yet, as happens
/// when the value is inherited through `extends`.
#[test]
fn file_source_refuses_without_force_when_the_store_is_empty() -> Result<(), StepError> {
    use mriya::{InitConfig, InitRequest, ScalewayConfig, VolumeIdSource};

    use crate::test_constants::DEFAULT_INSTANCE_TYPE;

    let scaleway = ScalewayConfig {
        access_key: None,
        secret_key: String::from("secret"),
        default_organization_id: None,
        default_project_id: String::from("project"),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from(DEFAULT_INSTANCE_TYPE),
        default_image: String::from("ubuntu"),
        default_architecture: String::from("x86_64"),
        default_volume_id: Some(String::from("inherited-volume")),
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    };
    let request = InitRequest::from_config(
        &scaleway,
        &InitConfig { volume_size_gb: 20 },
        "project",
        false,
    )
    .map_err(|err| StepError::Assertion(err.to_string()))?;
    if request.volume_id_source() != &VolumeIdSource::File {
        return Err(StepError::Assertion(String::from(
            "a configured ID should produce a File source",
        )));
    }
    let (orchestrator, backend) = orchestrator_with_empty_store()?;
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;

    let result = runtime.block_on(async { orchestrator.execute(&request).await });

    if !matches!(
        result,
        Err(mriya::InitError::FileVolumeAlreadyConfigured { .. })
    ) {
        return Err(StepError::Assertion(format!(
            "expected a file-backed refusal, got {result:?}"
        )));
    }
    if backend.create_volume_calls() != 0 {
        return Err(StepError::Assertion(String::from(
            "the guard must refuse before creating a volume",
        )));
    }
    Ok(())
}

/// The public constructor plus the source setter refuses an environment value
/// even with `--force`, with no store entry to compare against.
#[test]
fn environment_source_refuses_with_force_through_the_public_constructor() -> Result<(), StepError> {
    use mriya::{InitConfig, InitRequest, ScalewayConfig, VolumeIdSource};

    use crate::test_constants::DEFAULT_INSTANCE_TYPE;

    let scaleway = ScalewayConfig {
        access_key: None,
        secret_key: String::from("secret"),
        default_organization_id: None,
        default_project_id: String::from("project"),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from(DEFAULT_INSTANCE_TYPE),
        default_image: String::from("ubuntu"),
        default_architecture: String::from("x86_64"),
        default_volume_id: None,
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    };
    let request = InitRequest::from_config(
        &scaleway,
        &InitConfig { volume_size_gb: 20 },
        "project",
        true,
    )
    .map_err(|err| StepError::Assertion(err.to_string()))?
    .with_volume_id_source(VolumeIdSource::Environment(String::from("env-volume")));
    let (orchestrator, backend) = orchestrator_with_empty_store()?;
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;

    let result = runtime.block_on(async { orchestrator.execute(&request).await });

    if !matches!(result, Err(mriya::InitError::EnvironmentVolumeId { .. })) {
        return Err(StepError::Assertion(format!(
            "expected the environment guard, got {result:?}"
        )));
    }
    if backend.create_volume_calls() != 0 {
        return Err(StepError::Assertion(String::from(
            "the guard must refuse before creating a volume",
        )));
    }
    Ok(())
}

/// An inherited file ID is refused without `--force` when the selected child
/// file holds no ID and the environment supplies none. The composition path
/// must preserve the file provenance from `from_config`.
#[test]
fn inherited_file_id_is_refused_through_the_composition_path() -> Result<(), StepError> {
    use std::sync::Arc;

    use mriya::{
        InitConfig,
        InitError,
        ScalewayConfig,
        VolumeIdSource,
        request_from_loaded_config,
    };
    use ortho_config::{MapEnv, OrthoConfig as _};

    let tmp = tempfile::TempDir::new().map_err(|err| StepError::Assertion(err.to_string()))?;
    let root = camino::Utf8Path::from_path(tmp.path())
        .ok_or_else(|| StepError::Assertion(String::from("temp path should be UTF-8")))?;
    let child = root.join("mriya.toml");
    let dir = cap_std::fs_utf8::Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .map_err(|err| StepError::Assertion(err.to_string()))?;
    dir.write(
        "parent.toml",
        concat!(
            "secret_key = \"secret\"\n",
            "default_project_id = \"project\"\n",
            "default_volume_id = \"inherited-volume\"\n"
        ),
    )
    .map_err(|err| StepError::Assertion(err.to_string()))?;
    dir.write("mriya.toml", "extends = \"parent.toml\"\n")
        .map_err(|err| StepError::Assertion(err.to_string()))?;
    let env = MapEnv::new()
        .with_current_dir(root)
        .with_var("MRIYA_CONFIG_PATH", child.as_str());
    let shared = Arc::new(env);
    let scaleway =
        ScalewayConfig::load_from_iter_with_sources(["mriya"], shared.clone(), shared.clone())
            .map_err(|err| StepError::Assertion(err.to_string()))?;
    let request = request_from_loaded_config(
        mriya::InitRequestInputs {
            scaleway: &scaleway,
            init_config: &InitConfig { volume_size_gb: 20 },
            project_name: "project",
            overwrite_existing_volume_id: false,
        },
        shared.as_ref(),
    )
    .map_err(|err| StepError::Assertion(err.to_string()))?;
    if request.volume_id_source() != &VolumeIdSource::File {
        return Err(StepError::Assertion(format!(
            "inherited ID should keep a File source, got {:?}",
            request.volume_id_source()
        )));
    }
    let (orchestrator, backend) = orchestrator_with_empty_store()?;
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;

    let result = runtime.block_on(async { orchestrator.execute(&request).await });

    if !matches!(result, Err(InitError::FileVolumeAlreadyConfigured { .. })) {
        return Err(StepError::Assertion(format!(
            "expected a file-backed refusal, got {result:?}"
        )));
    }
    if backend.create_volume_calls() != 0 {
        return Err(StepError::Assertion(String::from(
            "the guard must refuse before creating a volume",
        )));
    }
    Ok(())
}
