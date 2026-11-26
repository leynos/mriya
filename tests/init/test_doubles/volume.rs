//! [`VolumeBackend`] implementation for the scripted init backend.

use mriya::{
    InstanceHandle,
    TeardownFailure,
    VolumeBackend,
    VolumeHandle,
    VolumeRequest,
    VolumeState,
    VolumeStatus,
    backend::BackendFuture,
};

use super::{
    FailureMode,
    LifecycleEvent,
    OperationSpec,
    ScriptedVolumeBackend,
    ScriptedVolumeBackendError,
};
use crate::init::doubles_support::volume_state;

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
                state.events.push(LifecycleEvent::VolumeReady);
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
                state.events.push(LifecycleEvent::VolumeDeleted);
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
    ) -> BackendFuture<'a, (), TeardownFailure<Self::Error>> {
        Box::pin(async move { self.run_teardown() })
    }
}
