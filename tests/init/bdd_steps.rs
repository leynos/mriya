//! BDD step definitions for the `mriya init` workflow.

use mriya::sync::Syncer;
use mriya::{InitError, InitOrchestrator};
use rstest_bdd_macros::{given, then, when};
use tokio::runtime::Runtime;

use super::test_doubles::MemoryConfigStore;
use super::test_helpers::{
    InitContext, InitContextResult, InitFailure, InitFailureKind, InitResult, InitTestError,
};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error(transparent)]
    Setup(#[from] InitTestError),
    #[error("assertion failed: {0}")]
    Assertion(String),
}

#[given("a ready init workflow")]
fn ready_workflow(init_context_result: InitContextResult) -> Result<InitContextResult, StepError> {
    let init_context = init_context_result?;
    Ok(Ok(init_context))
}

#[given("the formatter succeeds")]
fn formatter_succeeds(
    init_context_result: InitContextResult,
) -> Result<InitContextResult, StepError> {
    let init_context = init_context_result?;
    init_context.runner.push_success();
    Ok(Ok(init_context))
}

#[given("the formatter fails with exit code \"{code}\"")]
fn formatter_fails(
    init_context_result: InitContextResult,
    code: i32,
) -> Result<InitContextResult, StepError> {
    let init_context = init_context_result?;
    init_context.runner.push_failure(code);
    Ok(Ok(init_context))
}

#[given("volume creation fails")]
fn volume_creation_fails(
    init_context_result: InitContextResult,
) -> Result<InitContextResult, StepError> {
    let init_context = init_context_result?;
    init_context.backend.fail_create_volume();
    Ok(Ok(init_context))
}

#[given("instance provisioning fails")]
fn instance_provisioning_fails(
    init_context_result: InitContextResult,
) -> Result<InitContextResult, StepError> {
    let init_context = init_context_result?;
    init_context.backend.fail_provision();
    Ok(Ok(init_context))
}

#[given("instance readiness fails")]
fn instance_readiness_fails(
    init_context_result: InitContextResult,
) -> Result<InitContextResult, StepError> {
    let init_context = init_context_result?;
    init_context.backend.fail_wait();
    Ok(Ok(init_context))
}

#[given("volume detachment fails")]
fn volume_detachment_fails(
    init_context_result: InitContextResult,
) -> Result<InitContextResult, StepError> {
    let init_context = init_context_result?;
    init_context.backend.fail_detach();
    Ok(Ok(init_context))
}

#[given("teardown fails")]
fn teardown_fails(init_context_result: InitContextResult) -> Result<InitContextResult, StepError> {
    let init_context = init_context_result?;
    init_context.backend.fail_destroy();
    Ok(Ok(init_context))
}

#[given("configuration already contains a volume id")]
fn config_contains_volume_id(
    init_context_result: InitContextResult,
) -> Result<InitContextResult, StepError> {
    let mut init_context = init_context_result?;
    init_context.config_store = MemoryConfigStore::with_existing("vol-existing");
    Ok(Ok(init_context))
}

#[given("force overwrite is enabled")]
fn force_overwrite_enabled(
    init_context_result: InitContextResult,
) -> Result<InitContextResult, StepError> {
    let mut init_context = init_context_result?;
    init_context.request.overwrite_existing_volume_id = true;
    Ok(Ok(init_context))
}

#[when("I prepare the cache volume")]
fn prepare_volume(init_context_result: InitContextResult) -> Result<InitContextResult, StepError> {
    let runtime = Runtime::new().map_err(|err| StepError::Assertion(err.to_string()))?;
    let init_context = init_context_result?;
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

    Ok(Ok(InitContext {
        backend,
        runner,
        sync_config,
        request,
        config_store,
        outcome: Some(outcome),
    }))
}

#[then("the init result is successful")]
fn init_success(init_context_result: &InitContextResult) -> Result<(), StepError> {
    let init_context = init_context_result
        .as_ref()
        .map_err(|err| StepError::Assertion(err.to_string()))?;
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
fn init_error_kind(init_context_result: &InitContextResult, kind: String) -> Result<(), StepError> {
    let init_context = init_context_result
        .as_ref()
        .map_err(|err| StepError::Assertion(err.to_string()))?;
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
fn volume_formatted(init_context_result: &InitContextResult) -> Result<(), StepError> {
    let init_context = init_context_result
        .as_ref()
        .map_err(|err| StepError::Assertion(err.to_string()))?;
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

fn assert_counter_condition(
    init_context_result: &InitContextResult,
    get_counter: impl FnOnce(&InitContext) -> u32,
    predicate: impl FnOnce(u32) -> bool,
    error_message: &str,
) -> Result<(), StepError> {
    let init_context = init_context_result
        .as_ref()
        .map_err(|err| StepError::Assertion(err.to_string()))?;
    let count = get_counter(init_context);
    if predicate(count) {
        Ok(())
    } else {
        Err(StepError::Assertion(String::from(error_message)))
    }
}

#[then("the config is updated")]
fn config_updated(init_context_result: &InitContextResult) -> Result<(), StepError> {
    assert_counter_condition(
        init_context_result,
        |ctx| ctx.config_store.write_calls(),
        |count| count > 0,
        "config writer should be invoked",
    )
}

#[then("the instance is destroyed")]
fn instance_destroyed(init_context_result: &InitContextResult) -> Result<(), StepError> {
    assert_counter_condition(
        init_context_result,
        |ctx| ctx.backend.destroy_calls(),
        |count| count > 0,
        "backend.destroy should be invoked",
    )
}

#[then("the volume is not created")]
fn volume_not_created(init_context_result: &InitContextResult) -> Result<(), StepError> {
    assert_counter_condition(
        init_context_result,
        |ctx| ctx.backend.create_volume_calls(),
        |count| count == 0,
        "volume should not be created",
    )
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
