//! Test doubles for the run orchestrator.
//!
//! Provides a scripted backend that records teardown attempts and allows
//! controlled failures for create, wait, and destroy phases.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};

use mriya::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest, backend::BackendFuture};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct ScriptedBackend {
    state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    fail_on_destroy: bool,
    fail_on_create: bool,
    fail_on_wait: bool,
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
            .unwrap_or_else(|err| panic!("scripted backend lock poisoned: {err}"))
            .fail_on_destroy = true;
    }

    #[allow(dead_code, reason = "used by future failure-path tests")]
    pub fn fail_on_create(&self) {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("scripted backend lock poisoned: {err}"))
            .fail_on_create = true;
    }

    #[allow(dead_code, reason = "used by future failure-path tests")]
    pub fn fail_on_wait(&self) {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("scripted backend lock poisoned: {err}"))
            .fail_on_wait = true;
    }

    pub fn destroy_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|err| panic!("scripted backend lock poisoned: {err}"))
            .destroy_calls
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScriptedBackendError {
    #[error("create failure")]
    Create,
    #[error("wait failure")]
    Wait,
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
            if self
                .state
                .lock()
                .unwrap_or_else(|err| panic!("scripted backend lock poisoned: {err}"))
                .fail_on_create
            {
                Err(ScriptedBackendError::Create)
            } else {
                Ok(InstanceHandle {
                    id: String::from("scripted-id"),
                    zone: String::from("test-zone"),
                })
            }
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        Box::pin(async move {
            if self
                .state
                .lock()
                .unwrap_or_else(|err| panic!("scripted backend lock poisoned: {err}"))
                .fail_on_wait
            {
                Err(ScriptedBackendError::Wait)
            } else {
                Ok(InstanceNetworking {
                    public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    ssh_port: 22,
                })
            }
        })
    }

    fn destroy(&self, _handle: InstanceHandle) -> BackendFuture<'_, (), Self::Error> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| ScriptedBackendError::Destroy)?;
            state.destroy_calls += 1;
            if state.fail_on_destroy {
                return Err(ScriptedBackendError::Destroy);
            }
            Ok(())
        })
    }
}
