//! Test doubles for the run orchestrator.
//!
//! Provides a scripted backend that records teardown attempts and allows
//! controlled failure of the destroy phase to exercise teardown handling. The
//! volume operations are inert: the run orchestrator uses them to detach a
//! configured cache, and the double records the volume ID it was asked to
//! detach so ordering assertions can prove the cache is not deleted with the
//! server.

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
};

use mriya::{
    Backend,
    InstanceHandle,
    InstanceNetworking,
    InstanceRequest,
    TeardownFailure,
    TeardownStage,
    VolumeBackend,
    VolumeHandle,
    VolumeRequest,
    VolumeState,
    VolumeStatus,
    backend::BackendFuture,
};
use thiserror::Error;

/// Test double for [`Backend`] and [`VolumeBackend`] that wraps shared
/// [`State`] to record teardown attempts and optionally simulate a destroy
/// failure.
#[derive(Clone, Debug)]
pub struct ScriptedBackend {
    state: Arc<Mutex<State>>,
    event_log: Option<Arc<Mutex<Vec<String>>>>,
}

/// Internal mutable state tracked by the scripted backend.
#[derive(Debug, Default)]
struct State {
    fail_on_destroy: bool,
    destroy_calls: u32,
    teardown_volume_id: Option<String>,
}

impl ScriptedBackend {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            event_log: None,
        }
    }

    /// Creates a backend that appends lifecycle events to a shared log.
    pub fn with_event_log(event_log: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            event_log: Some(event_log),
            ..Self::new()
        }
    }

    /// Records one lifecycle event when a shared log is attached.
    fn record(&self, event: &str) {
        if let Some(log) = &self.event_log
            && let Ok(mut events) = log.lock()
        {
            events.push(event.to_owned());
        }
    }

    /// Locks the shared state, recovering from a poisoned mutex.
    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn fail_on_destroy(&self) { self.lock_state().fail_on_destroy = true; }

    pub fn destroy_calls(&self) -> u32 { self.lock_state().destroy_calls }

    pub fn teardown_volume_id(&self) -> Option<String> {
        self.lock_state().teardown_volume_id.clone()
    }
}

/// Error variants surfaced by [`ScriptedBackend`], modelling scripted failure
/// points in backend lifecycle operations.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScriptedBackendError {
    /// Raised when the scripted backend is instructed to fail teardown.
    #[error("destroy failure")]
    Destroy,
}

/// Wraps an already-computed value in a ready backend future.
fn immediate<'a, T: Send + 'a>(value: T) -> BackendFuture<'a, T, ScriptedBackendError> {
    Box::pin(async move { Ok(value) })
}

impl Backend for ScriptedBackend {
    type Error = ScriptedBackendError;

    fn create<'a>(
        &'a self,
        _request: &'a InstanceRequest,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        immediate(InstanceHandle {
            id: String::from("scripted-id"),
            zone: String::from("test-zone"),
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        immediate(InstanceNetworking {
            public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            ssh_port: 22,
        })
    }

    fn destroy(&self, _handle: InstanceHandle) -> BackendFuture<'_, (), Self::Error> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.destroy_calls += 1;
            if state.fail_on_destroy {
                return Err(ScriptedBackendError::Destroy);
            }
            Ok(())
        })
    }
}

impl VolumeBackend for ScriptedBackend {
    fn create_volume<'a>(
        &'a self,
        _request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error> {
        immediate(VolumeHandle {
            id: String::from("vol-123"),
            zone: String::from("test-zone"),
        })
    }

    fn get_volume<'a>(
        &'a self,
        _zone: &'a str,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, VolumeState, Self::Error> {
        immediate(volume_state())
    }

    fn wait_for_volume_status<'a>(
        &'a self,
        _zone: &'a str,
        _volume_id: &'a str,
        _status: VolumeStatus,
    ) -> BackendFuture<'a, VolumeState, Self::Error> {
        immediate(volume_state())
    }

    fn delete_volume<'a>(
        &'a self,
        _handle: &'a VolumeHandle,
    ) -> BackendFuture<'a, (), Self::Error> {
        immediate(())
    }

    fn attach_volume<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        immediate(())
    }

    fn detach_volume<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        immediate(())
    }

    fn teardown<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        volume_id: Option<&'a str>,
    ) -> BackendFuture<'a, (), TeardownFailure<Self::Error>> {
        Box::pin(async move {
            self.record("teardown");
            let mut state = self.lock_state();
            state.destroy_calls += 1;
            state.teardown_volume_id = volume_id.map(str::to_owned);
            if state.fail_on_destroy {
                return Err(TeardownFailure {
                    stage: TeardownStage::Destroying,
                    source: ScriptedBackendError::Destroy,
                });
            }
            Ok(())
        })
    }
}

/// Builds the available volume state the double reports.
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
