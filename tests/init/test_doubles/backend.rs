//! [`Backend`] implementation for the scripted init backend.

use std::net::{IpAddr, Ipv4Addr};

use mriya::{Backend, InstanceHandle, InstanceNetworking, InstanceRequest, backend::BackendFuture};

use super::{
    FailureMode,
    LifecycleEvent,
    OperationHooks,
    OperationSpec,
    ScriptedVolumeBackend,
    ScriptedVolumeBackendError,
    State,
};

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
            OperationHooks {
                on_attempt: |_: &mut State| {},
                on_success: |state: &mut State| {
                    state.events.push(LifecycleEvent::InstanceCreated);
                },
            },
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
            OperationHooks {
                on_attempt: |_: &mut State| {},
                on_success: |_: &mut State| {},
            },
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
            OperationHooks {
                on_attempt: |state: &mut State| {
                    state.destroy_calls += 1;
                },
                on_success: |_: &mut State| {},
            },
            || (),
        )
    }
}
