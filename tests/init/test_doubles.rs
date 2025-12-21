//! Test doubles for init workflow scenarios.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};

use camino::Utf8PathBuf;
use mriya::backend::BackendFuture;
use mriya::{
    Backend, ConfigStoreError, ConfigWriter, InstanceHandle, InstanceNetworking, InstanceRequest,
    VolumeBackend, VolumeHandle, VolumeRequest,
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
    Provision,
    Wait,
    Detach,
    Destroy,
}

impl FailureMode {
    const fn flag(self) -> u8 {
        match self {
            Self::CreateVolume => 0b00001,
            Self::Provision => 0b00010,
            Self::Wait => 0b00100,
            Self::Detach => 0b01000,
            Self::Destroy => 0b10000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Failures(u8);

impl Failures {
    const fn set(&mut self, mode: FailureMode) {
        self.0 |= mode.flag();
    }

    const fn contains(self, mode: FailureMode) -> bool {
        self.0 & mode.flag() != 0
    }
}

#[derive(Debug, Default)]
struct State {
    failures: Failures,
    create_volume_calls: u32,
    destroy_calls: u32,
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
            .unwrap_or_else(|err| panic!("lock poisoned: fail_create_volume: {err}"))
            .failures
            .set(FailureMode::CreateVolume);
    }

    pub fn fail_provision(&self) {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("lock poisoned: fail_provision: {err}"))
            .failures
            .set(FailureMode::Provision);
    }

    pub fn fail_wait(&self) {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("lock poisoned: fail_wait: {err}"))
            .failures
            .set(FailureMode::Wait);
    }

    pub fn fail_detach(&self) {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("lock poisoned: fail_detach: {err}"))
            .failures
            .set(FailureMode::Detach);
    }

    pub fn fail_destroy(&self) {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("lock poisoned: fail_destroy: {err}"))
            .failures
            .set(FailureMode::Destroy);
    }

    pub fn create_volume_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("lock poisoned: create_volume_calls: {err}"))
            .create_volume_calls
    }

    pub fn destroy_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("lock poisoned: destroy_calls: {err}"))
            .destroy_calls
    }
}

/// Errors raised by the scripted backend to model failure points.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScriptedVolumeBackendError {
    #[error("volume create failure")]
    VolumeCreate,
    #[error("provision failure")]
    Provision,
    #[error("wait failure")]
    Wait,
    #[error("detach failure")]
    Detach,
    #[error("destroy failure")]
    Destroy,
}

impl Backend for ScriptedVolumeBackend {
    type Error = ScriptedVolumeBackendError;

    fn create<'a>(
        &'a self,
        _request: &'a InstanceRequest,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|err| panic!("lock poisoned in create: {err}"));
            if state.failures.contains(FailureMode::Provision) {
                return Err(ScriptedVolumeBackendError::Provision);
            }
            Ok(InstanceHandle {
                id: String::from("instance-123"),
                zone: String::from("test-zone"),
            })
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|err| panic!("lock poisoned in wait: {err}"));
            if state.failures.contains(FailureMode::Wait) {
                return Err(ScriptedVolumeBackendError::Wait);
            }
            Ok(InstanceNetworking {
                public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                ssh_port: 22,
            })
        })
    }

    fn destroy(&self, _handle: InstanceHandle) -> BackendFuture<'_, (), Self::Error> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|err| panic!("lock poisoned in destroy: {err}"));
            state.destroy_calls += 1;
            if state.failures.contains(FailureMode::Destroy) {
                return Err(ScriptedVolumeBackendError::Destroy);
            }
            Ok(())
        })
    }
}

impl VolumeBackend for ScriptedVolumeBackend {
    fn create_volume<'a>(
        &'a self,
        _request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|err| panic!("lock poisoned in create_volume: {err}"));
            state.create_volume_calls += 1;
            if state.failures.contains(FailureMode::CreateVolume) {
                return Err(ScriptedVolumeBackendError::VolumeCreate);
            }
            Ok(VolumeHandle {
                id: String::from("vol-123"),
                zone: String::from("test-zone"),
            })
        })
    }

    fn detach_volume<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|err| panic!("lock poisoned in detach: {err}"));
            if state.failures.contains(FailureMode::Detach) {
                return Err(ScriptedVolumeBackendError::Detach);
            }
            Ok(())
        })
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
            .unwrap_or_else(|err| panic!("lock poisoned: with_existing: {err}"))
            .existing_volume_id = Some(volume_id.to_owned());
        store
    }

    pub fn write_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("lock poisoned: write_calls: {err}"))
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
        if let Some(existing) = state.existing_volume_id.clone()
            && !force
        {
            return Err(ConfigStoreError::VolumeAlreadyConfigured {
                volume_id: existing,
            });
        }
        state.existing_volume_id = Some(volume_id.to_owned());
        state.write_calls += 1;
        Ok(Utf8PathBuf::from("mriya.toml"))
    }
}
