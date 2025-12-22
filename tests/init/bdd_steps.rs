//! BDD step definitions for the `mriya init` workflow.

use mriya::sync::Syncer;
use mriya::{InitError, InitOrchestrator};
use rstest_bdd_macros::{given, then, when};
use tokio::runtime::Runtime;

use super::test_doubles::MemoryConfigStore;
use super::test_helpers::{InitContext, InitFailure, InitFailureKind, InitResult, InitTestError};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error(transparent)]
    Setup(#[from] InitTestError),
    #[error("assertion failed: {0}")]
    Assertion(String),
}

#[given("a ready init workflow")]
fn ready_workflow(init_context: InitContext) -> InitContext {
    init_context
}

#[given("the formatter succeeds")]
fn formatter_succeeds(init_context: InitContext) -> InitContext {
    init_context.runner.push_success();
    init_context
}

#[given("the formatter fails with exit code \"{code}\"")]
fn formatter_fails(init_context: InitContext, code: i32) -> InitContext {
    init_context.runner.push_failure(code);
    init_context
}

#[given("volume creation fails")]
fn volume_creation_fails(init_context: InitContext) -> InitContext {
    init_context.backend.fail_create_volume();
    init_context
}

#[given("instance provisioning fails")]
fn instance_provisioning_fails(init_context: InitContext) -> InitContext {
    init_context.backend.fail_provision();
    init_context
}

#[given("instance readiness fails")]
fn instance_readiness_fails(init_context: InitContext) -> InitContext {
    init_context.backend.fail_wait();
    init_context
}

#[given("volume detachment fails")]
fn volume_detachment_fails(init_context: InitContext) -> InitContext {
    init_context.backend.fail_detach();
    init_context
}

#[given("teardown fails")]
fn teardown_fails(init_context: InitContext) -> InitContext {
    init_context.backend.fail_destroy();
    init_context
}

#[given("configuration already contains a volume id")]
fn config_contains_volume_id(mut init_context: InitContext) -> InitContext {
    init_context.config_store = MemoryConfigStore::with_existing("vol-existing");
    init_context
}

#[given("force overwrite is enabled")]
fn force_overwrite_enabled(mut init_context: InitContext) -> InitContext {
    init_context.request.overwrite_existing_volume_id = true;
    init_context
}

#[when("I prepare the cache volume")]
fn prepare_volume(init_context: InitContext) -> Result<InitContext, StepError> {
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;
    let InitContext {
        backend,
        runner,
        sync_config,
        request,
        config_store,
        ..
    } = init_context;

    let syncer = Syncer::new(sync_config.clone(), runner.clone())
        .map_err(InitTestError::from)
        .map_err(StepError::from)?;
    let orchestrator = InitOrchestrator::new(backend.clone(), syncer, config_store.clone());

    let request_clone = request.clone();
    let result = runtime.block_on(async move { orchestrator.execute(&request_clone).await });
    let outcome = match result {
        Ok(_) => InitResult::Success,
        Err(err) => InitResult::Failure(InitFailure {
            kind: map_failure_kind(&err),
            message: err.to_string(),
        }),
    };

    Ok(InitContext {
        backend,
        runner,
        sync_config,
        request,
        config_store,
        outcome: Some(outcome),
    })
}

#[then("the init result is successful")]
fn init_success(init_context: &InitContext) -> Result<(), StepError> {
    match init_context.outcome {
        Some(InitResult::Success) => Ok(()),
        Some(InitResult::Failure(ref failure)) => Err(StepError::Assertion(format!(
            "expected success, got failure: {}",
            failure.message
        ))),
        None => Err(StepError::Assertion(String::from("missing outcome"))),
    }
}

#[then("the init error kind is \"{kind}\"")]
fn init_error_kind(init_context: &InitContext, kind: String) -> Result<(), StepError> {
    let expected = parse_failure_kind(&kind)?;
    let Some(InitResult::Failure(failure)) = &init_context.outcome else {
        return Err(StepError::Assertion(String::from(
            "expected failure outcome",
        )));
    };
    if failure.kind == expected {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected failure kind {expected:?}, got {:?}",
            failure.kind
        )))
    }
}

#[then("the volume is formatted")]
fn volume_formatted(init_context: &InitContext) -> Result<(), StepError> {
    let invocations = init_context.runner.invocations();
    let invocation = invocations
        .first()
        .ok_or_else(|| StepError::Assertion(String::from("missing ssh invocation")))?;
    let command = invocation.command_string();
    let expected_path = "/dev/disk/by-id/scsi-0SCW_BSSD_vol-123";
    if command.contains("mkfs.ext4") && command.contains(expected_path) {
        Ok(())
    } else {
        Err(StepError::Assertion(format!(
            "expected mkfs.ext4 command for {expected_path}, got: {command}"
        )))
    }
}

#[then("the config is updated")]
fn config_updated(init_context: &InitContext) -> Result<(), StepError> {
    let calls = init_context.config_store.write_calls();
    if calls == 0 {
        return Err(StepError::Assertion(String::from(
            "config writer should be invoked",
        )));
    }
    Ok(())
}

#[then("the instance is destroyed")]
fn instance_destroyed(init_context: &InitContext) -> Result<(), StepError> {
    if init_context.backend.destroy_calls() > 0 {
        Ok(())
    } else {
        Err(StepError::Assertion(String::from(
            "backend.destroy should be invoked",
        )))
    }
}

#[then("the volume is not created")]
fn volume_not_created(init_context: &InitContext) -> Result<(), StepError> {
    if init_context.backend.create_volume_calls() == 0 {
        Ok(())
    } else {
        Err(StepError::Assertion(String::from(
            "volume should not be created",
        )))
    }
}

const fn map_failure_kind(
    err: &InitError<super::test_doubles::ScriptedVolumeBackendError>,
) -> InitFailureKind {
    match err {
        InitError::Config(_) => InitFailureKind::Config,
        InitError::Volume(_) => InitFailureKind::Volume,
        InitError::Provision(_) => InitFailureKind::Provision,
        InitError::Wait { .. } => InitFailureKind::Wait,
        InitError::Format { .. } => InitFailureKind::Format,
        InitError::Detach { .. } => InitFailureKind::Detach,
        InitError::Teardown(_) => InitFailureKind::Teardown,
    }
}

fn parse_failure_kind(kind: &str) -> Result<InitFailureKind, StepError> {
    match kind {
        "config" => Ok(InitFailureKind::Config),
        "volume" => Ok(InitFailureKind::Volume),
        "provision" => Ok(InitFailureKind::Provision),
        "wait" => Ok(InitFailureKind::Wait),
        "format" => Ok(InitFailureKind::Format),
        "detach" => Ok(InitFailureKind::Detach),
        "teardown" => Ok(InitFailureKind::Teardown),
        _ => Err(StepError::Assertion(format!(
            "unknown failure kind: {kind}"
        ))),
    }
}
