//! Test doubles for init workflow scenarios.

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
};

use camino::Utf8PathBuf;
use mriya::{
    Backend,
    ConfigStoreError,
    ConfigWriter,
    InstanceHandle,
    InstanceNetworking,
    InstanceRequest,
    VolumeBackend,
    VolumeHandle,
    VolumeRequest,
    VolumeState,
    VolumeStatus,
    backend::BackendFuture,
};
use thiserror::Error;

/// Scripted backend that simulates volume and instance lifecycle operations.
#[derive(Clone, Debug)]
pub struct ScriptedVolumeBackend {
    state: Arc<Mutex<State>>,
}

#[derive(Clone, Copy, Debug)]
enum FailureMode {
    CreateVolume,
    VolumeReady,
    Provision,
    Wait,
    Detach,
    Destroy,
    DeleteVolume,
}

impl FailureMode {
    const fn flag(self) -> u8 {
        match self {
            Self::CreateVolume => 0b0000_0001,
            Self::VolumeReady => 0b0000_0010,
            Self::Provision => 0b0000_0100,
            Self::Wait => 0b0000_1000,
            Self::Detach => 0b0001_0000,
            Self::Destroy => 0b0010_0000,
            Self::DeleteVolume => 0b0100_0000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Failures(u8);

impl Failures {
    const fn set(&mut self, mode: FailureMode) { self.0 |= mode.flag(); }

    const fn contains(self, mode: FailureMode) -> bool { self.0 & mode.flag() != 0 }
}

#[derive(Debug, Default)]
struct State {
    failures: Failures,
    create_volume_calls: u32,
    volume_ready_calls: u32,
    delete_volume_calls: u32,
    teardown_calls: u32,
    destroy_calls: u32,
}

/// Builds the available volume state the doubles report.
fn volume_state() -> VolumeState {
    VolumeState {
        id: String::from("vol-123"),
        zone: String::from("test-zone"),
        project_id: String::from("project"),
        status: VolumeStatus::Available,
        volume_type: String::from("sbs_5k"),
        storage_class: Some(String::from("sbs")),
        size_bytes: 10_000_000_000,
        references: Vec::new(),
    }
}

#[derive(Clone)]
struct OperationSpec {
    context: &'static str,
    failure_mode: FailureMode,
    error: ScriptedVolumeBackendError,
}

impl ScriptedVolumeBackend {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn fail_create_volume(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .set(FailureMode::CreateVolume);
    }

    pub fn fail_volume_ready(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .set(FailureMode::VolumeReady);
    }

    pub fn fail_delete_volume(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .set(FailureMode::DeleteVolume);
    }

    pub fn fail_provision(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .set(FailureMode::Provision);
    }

    pub fn fail_wait(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .set(FailureMode::Wait);
    }

    pub fn fail_detach(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .set(FailureMode::Detach);
    }

    pub fn fail_destroy(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .set(FailureMode::Destroy);
    }

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

    pub fn volume_ready_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .volume_ready_calls
    }

    pub fn teardown_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .teardown_calls
    }

    /// Records one ordered-teardown attempt and reports its scripted failure.
    fn run_teardown(&self) -> Result<(), ScriptedVolumeBackendError> {
        let mut state = self.state.lock().map_err(|err| {
            ScriptedVolumeBackendError::Lock(format!("lock poisoned in teardown: {err}"))
        })?;
        state.teardown_calls += 1;
        state.destroy_calls += 1;

        if state.failures.contains(FailureMode::Detach) {
            return Err(ScriptedVolumeBackendError::Detach);
        }
        if state.failures.contains(FailureMode::Destroy) {
            return Err(ScriptedVolumeBackendError::Destroy);
        }
        Ok(())
    }

    /// Execute an operation with scripted failure checking.
    fn execute_with_failure_check<T>(
        &self,
        operation: OperationSpec,
        increment: impl FnOnce(&mut State),
        success: impl FnOnce() -> T,
    ) -> Result<T, ScriptedVolumeBackendError> {
        let mut state = self.state.lock().map_err(|err| {
            ScriptedVolumeBackendError::Lock(format!(
                "lock poisoned in {}: {err}",
                operation.context
            ))
        })?;

        increment(&mut state);

        if state.failures.contains(operation.failure_mode) {
            return Err(operation.error);
        }

        Ok(success())
    }

    fn run_operation<'a, T>(
        &'a self,
        operation: OperationSpec,
        increment: impl FnOnce(&mut State) + Send + 'a,
        success: impl FnOnce() -> T + Send + 'a,
    ) -> BackendFuture<'a, T, ScriptedVolumeBackendError>
    where
        T: Send + 'a,
    {
        Box::pin(async move { self.execute_with_failure_check(operation, increment, success) })
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

impl Backend for ScriptedVolumeBackend {
    type Error = ScriptedVolumeBackendError;

    fn create<'a>(
        &'a self,
        _request: &'a InstanceRequest,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "create",
                failure_mode: FailureMode::Provision,
                error: ScriptedVolumeBackendError::Provision,
            },
            |_| {},
            || InstanceHandle {
                id: String::from("instance-123"),
                zone: String::from("test-zone"),
            },
        )
    }

    fn wait_for_ready<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "wait_for_ready",
                failure_mode: FailureMode::Wait,
                error: ScriptedVolumeBackendError::Wait,
            },
            |_| {},
            || InstanceNetworking {
                public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                ssh_port: 22,
            },
        )
    }

    fn destroy(&self, _handle: InstanceHandle) -> BackendFuture<'_, (), Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "destroy",
                failure_mode: FailureMode::Destroy,
                error: ScriptedVolumeBackendError::Destroy,
            },
            |state| {
                state.destroy_calls += 1;
            },
            || (),
        )
    }
}

impl VolumeBackend for ScriptedVolumeBackend {
    fn create_volume<'a>(
        &'a self,
        _request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "create_volume",
                failure_mode: FailureMode::CreateVolume,
                error: ScriptedVolumeBackendError::VolumeCreate,
            },
            |state| {
                state.create_volume_calls += 1;
            },
            || VolumeHandle {
                id: String::from("vol-123"),
                zone: String::from("test-zone"),
            },
        )
    }

    fn get_volume<'a>(
        &'a self,
        _zone: &'a str,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, VolumeState, Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "get_volume",
                failure_mode: FailureMode::VolumeReady,
                error: ScriptedVolumeBackendError::VolumeReady,
            },
            |_| {},
            volume_state,
        )
    }

    fn wait_for_volume_status<'a>(
        &'a self,
        _zone: &'a str,
        _volume_id: &'a str,
        _status: VolumeStatus,
    ) -> BackendFuture<'a, VolumeState, Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "wait_for_volume_status",
                failure_mode: FailureMode::VolumeReady,
                error: ScriptedVolumeBackendError::VolumeReady,
            },
            |state| {
                state.volume_ready_calls += 1;
            },
            volume_state,
        )
    }

    fn delete_volume<'a>(
        &'a self,
        _handle: &'a VolumeHandle,
    ) -> BackendFuture<'a, (), Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "delete_volume",
                failure_mode: FailureMode::DeleteVolume,
                error: ScriptedVolumeBackendError::DeleteVolume,
            },
            |state| {
                state.delete_volume_calls += 1;
            },
            || (),
        )
    }

    fn attach_volume<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "attach_volume",
                failure_mode: FailureMode::Provision,
                error: ScriptedVolumeBackendError::Provision,
            },
            |_| {},
            || (),
        )
    }

    fn detach_volume<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        self.run_operation(
            OperationSpec {
                context: "detach_volume",
                failure_mode: FailureMode::Detach,
                error: ScriptedVolumeBackendError::Detach,
            },
            |_| {},
            || (),
        )
    }

    fn teardown<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: Option<&'a str>,
    ) -> BackendFuture<'a, (), Self::Error> {
        Box::pin(async move { self.run_teardown() })
    }
}

/// In-memory config store for behavioural tests.
#[derive(Clone, Debug)]
pub struct MemoryConfigStore {
    state: Arc<Mutex<ConfigState>>,
}

#[derive(Debug, Default)]
struct ConfigState {
    existing_volume_id: Option<String>,
    write_calls: u32,
}

impl MemoryConfigStore {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ConfigState::default())),
        }
    }

    pub fn with_existing(volume_id: &str) -> Self {
        let store = Self::new();
        store
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .existing_volume_id = Some(volume_id.to_owned());
        store
    }

    pub fn write_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write_calls
    }

    fn lock_state(
        &self,
        context: &str,
    ) -> Result<std::sync::MutexGuard<'_, ConfigState>, ConfigStoreError> {
        self.state.lock().map_err(|err| ConfigStoreError::Io {
            path: Utf8PathBuf::from("in-memory"),
            message: format!("lock poisoned in {context}: {err}"),
        })
    }
}

impl ConfigWriter for MemoryConfigStore {
    fn current_volume_id(&self) -> Result<Option<String>, ConfigStoreError> {
        Ok(self
            .lock_state("current_volume_id")?
            .existing_volume_id
            .clone())
    }

    fn write_volume_id(
        &self,
        volume_id: &str,
        force: bool,
    ) -> Result<Utf8PathBuf, ConfigStoreError> {
        let mut state = self.lock_state("write_volume_id")?;
        if let Some(existing) = state.existing_volume_id.as_ref()
            && !force
        {
            return Err(ConfigStoreError::VolumeAlreadyConfigured {
                volume_id: existing.clone(),
            });
        }
        state.existing_volume_id = Some(volume_id.to_owned());
        state.write_calls += 1;
        Ok(Utf8PathBuf::from("mriya.toml"))
    }
}
