//! Test doubles for the run orchestrator.
//!
//! Provides a scripted backend that records teardown attempts and allows
//! controlled failure of the destroy phase to exercise teardown handling.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};

use mriya::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest, backend::BackendFuture};
use thiserror::Error;

/// Test double for [`Backend`] that wraps shared [`State`] to record teardown
/// attempts and optionally simulate a destroy failure for orchestrator tests.
#[derive(Clone, Debug)]
pub struct ScriptedBackend {
    state: Arc<Mutex<State>>,
}

/// Internal mutable state tracked by the scripted backend.
#[derive(Debug, Default)]
struct State {
    fail_on_destroy: bool,
    destroy_calls: u32,
}

impl ScriptedBackend {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn fail_on_destroy(&self) {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("scripted backend lock poisoned: fail_on_destroy: {err}"))
            .fail_on_destroy = true;
    }

    pub fn destroy_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("scripted backend lock poisoned: destroy_calls: {err}"))
            .destroy_calls
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

impl Backend for ScriptedBackend {
    type Error = ScriptedBackendError;

    fn create<'a>(
        &'a self,
        _request: &'a InstanceRequest,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        Box::pin(async move {
            Ok(InstanceHandle {
                id: String::from("scripted-id"),
                zone: String::from("test-zone"),
            })
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        Box::pin(async move {
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
                .unwrap_or_else(|err| panic!("scripted backend lock poisoned in destroy: {err}"));
            state.destroy_calls += 1;
            if state.fail_on_destroy {
                return Err(ScriptedBackendError::Destroy);
            }
            Ok(())
        })
    }
}
