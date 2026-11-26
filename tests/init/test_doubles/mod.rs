//! Test doubles for init workflow scenarios.

use std::sync::{Arc, Mutex};

use mriya::{TeardownFailure, TeardownStage, backend::BackendFuture};
use thiserror::Error;

use super::doubles_support::{FailureMode, Failures, LifecycleEvent};

/// Scripted backend that simulates volume and instance lifecycle operations.
#[derive(Clone, Debug)]
pub struct ScriptedVolumeBackend {
    state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    failures: Failures,
    create_volume_calls: u32,
    delete_volume_calls: u32,
    teardown_calls: u32,
    destroy_calls: u32,
    events: Vec<LifecycleEvent>,
}

#[derive(Clone)]
struct OperationSpec {
    context: &'static str,
    failure_mode: FailureMode,
    error: ScriptedVolumeBackendError,
}

/// Callbacks that record one scripted operation.
///
/// `on_attempt` runs before the scripted failure check, so counters observe a
/// failed call; `on_success` runs only after the check, so a completed-event
/// log never records a failure as a success.
struct OperationHooks<A, S> {
    on_attempt: A,
    on_success: S,
}

/// Hooks for the volume double, whose callbacks capture nothing.
type VolumeHooks = OperationHooks<fn(&mut State), fn(&mut State)>;

impl ScriptedVolumeBackend {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Marks one scripted failure mode as active.
    fn set_failure(&self, mode: FailureMode) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .set(mode);
    }

    pub fn fail_create_volume(&self) { self.set_failure(FailureMode::CreateVolume); }

    pub fn fail_volume_ready(&self) { self.set_failure(FailureMode::VolumeReady); }

    pub fn fail_delete_volume(&self) { self.set_failure(FailureMode::DeleteVolume); }

    pub fn fail_provision(&self) { self.set_failure(FailureMode::Provision); }

    pub fn fail_wait(&self) { self.set_failure(FailureMode::Wait); }

    pub fn fail_detach(&self) { self.set_failure(FailureMode::Detach); }

    pub fn fail_destroy(&self) { self.set_failure(FailureMode::Destroy); }

    pub fn create_volume_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .create_volume_calls
    }

    pub fn destroy_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .destroy_calls
    }

    pub fn delete_volume_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .delete_volume_calls
    }

    /// Returns the lifecycle events observed so far, in order.
    pub fn events(&self) -> Vec<LifecycleEvent> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .events
            .clone()
    }

    pub fn teardown_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .teardown_calls
    }

    /// Records one ordered-teardown attempt and reports its scripted failure.
    fn run_teardown(&self) -> Result<(), TeardownFailure<ScriptedVolumeBackendError>> {
        let mut state = self.state.lock().map_err(|err| TeardownFailure {
            stage: TeardownStage::Stopping,
            source: ScriptedVolumeBackendError::Lock(format!("lock poisoned in teardown: {err}")),
        })?;
        state.teardown_calls += 1;
        state.events.push(LifecycleEvent::Teardown);

        // A detach failure means the destroy is never attempted, so the
        // counter must stay put; a failed destroy attempt still counts.
        if state.failures.contains(FailureMode::Detach) {
            return Err(TeardownFailure {
                stage: TeardownStage::Detaching,
                source: ScriptedVolumeBackendError::Detach,
            });
        }
        state.destroy_calls += 1;
        if state.failures.contains(FailureMode::Destroy) {
            return Err(TeardownFailure {
                stage: TeardownStage::Destroying,
                source: ScriptedVolumeBackendError::Destroy,
            });
        }
        Ok(())
    }

    /// Execute an operation, recording the attempt and the completed event
    /// separately.
    ///
    /// The attempt counter runs before the scripted failure check so a failed
    /// call is still observed; the completed event runs only after the check
    /// passes, so a failure never records a success transition.
    fn execute_with_failure_check<T>(
        &self,
        operation: OperationSpec,
        hooks: OperationHooks<impl FnOnce(&mut State), impl FnOnce(&mut State)>,
        success: impl FnOnce() -> T,
    ) -> Result<T, ScriptedVolumeBackendError> {
        let mut state = self.state.lock().map_err(|err| {
            ScriptedVolumeBackendError::Lock(format!(
                "lock poisoned in {}: {err}",
                operation.context
            ))
        })?;

        (hooks.on_attempt)(&mut state);

        if state.failures.contains(operation.failure_mode) {
            return Err(operation.error);
        }

        (hooks.on_success)(&mut state);
        Ok(success())
    }

    fn run_operation<'a, T>(
        &'a self,
        operation: OperationSpec,
        hooks: OperationHooks<
            impl FnOnce(&mut State) + Send + 'a,
            impl FnOnce(&mut State) + Send + 'a,
        >,
        success: impl FnOnce() -> T + Send + 'a,
    ) -> BackendFuture<'a, T, ScriptedVolumeBackendError>
    where
        T: Send + 'a,
    {
        Box::pin(async move { self.execute_with_failure_check(operation, hooks, success) })
    }
}

/// Errors raised by the scripted backend to model failure points.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScriptedVolumeBackendError {
    #[error("volume create failure")]
    VolumeCreate,
    #[error("volume readiness failure")]
    VolumeReady,
    #[error("volume delete failure")]
    DeleteVolume,
    #[error("provision failure")]
    Provision,
    #[error("wait failure")]
    Wait,
    #[error("detach failure")]
    Detach,
    #[error("destroy failure")]
    Destroy,
    #[error("{0}")]
    Lock(String),
}

mod backend;
mod volume;
