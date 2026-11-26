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
    OperationHooks,
    OperationSpec,
    ScriptedVolumeBackend,
    ScriptedVolumeBackendError,
    VolumeHooks,
};
use crate::init::doubles_support::volume_state;

/// One scripted volume call and its recorded side effects.
#[derive(Clone, Copy)]
enum VolumeCall {
    /// Creates the scripted volume handle.
    Create,
    /// Reads the available volume state.
    Get,
    /// Waits for the volume and records readiness.
    WaitReady,
    /// Deletes the owned volume.
    Delete,
    /// Attaches the cache, with no recorded side effect.
    Attach,
    /// Detaches the cache, with no recorded side effect.
    Detach,
}

impl VolumeCall {
    /// Returns the operation spec for this call.
    const fn spec(self) -> OperationSpec {
        match self {
            Self::Create => OperationSpec {
                context: "create_volume",
                failure_mode: FailureMode::CreateVolume,
                error: ScriptedVolumeBackendError::VolumeCreate,
            },
            Self::Get => OperationSpec {
                context: "get_volume",
                failure_mode: FailureMode::VolumeReady,
                error: ScriptedVolumeBackendError::VolumeReady,
            },
            Self::WaitReady => OperationSpec {
                context: "wait_for_volume_status",
                failure_mode: FailureMode::VolumeReady,
                error: ScriptedVolumeBackendError::VolumeReady,
            },
            Self::Delete => OperationSpec {
                context: "delete_volume",
                failure_mode: FailureMode::DeleteVolume,
                error: ScriptedVolumeBackendError::DeleteVolume,
            },
            Self::Attach => OperationSpec {
                context: "attach_volume",
                failure_mode: FailureMode::Provision,
                error: ScriptedVolumeBackendError::Provision,
            },
            Self::Detach => OperationSpec {
                context: "detach_volume",
                failure_mode: FailureMode::Detach,
                error: ScriptedVolumeBackendError::Detach,
            },
        }
    }

    /// Returns the hooks recording this call's side effects.
    const fn hooks(self) -> VolumeHooks {
        match self {
            Self::Create => OperationHooks {
                on_attempt: |state| state.create_volume_calls += 1,
                on_success: |_| {},
            },
            Self::WaitReady => OperationHooks {
                on_attempt: |_| {},
                on_success: |state| state.events.push(LifecycleEvent::VolumeReady),
            },
            Self::Delete => OperationHooks {
                on_attempt: |state| state.delete_volume_calls += 1,
                on_success: |state| state.events.push(LifecycleEvent::VolumeDeleted),
            },
            Self::Get | Self::Attach | Self::Detach => OperationHooks {
                on_attempt: |_| {},
                on_success: |_| {},
            },
        }
    }
}

impl ScriptedVolumeBackend {
    /// Runs one scripted volume call, returning `success` on success.
    fn volume_call<'a, T>(
        &'a self,
        call: VolumeCall,
        success: impl FnOnce() -> T + Send + 'a,
    ) -> BackendFuture<'a, T, ScriptedVolumeBackendError>
    where
        T: Send + 'a,
    {
        self.run_operation(call.spec(), call.hooks(), success)
    }
}

impl VolumeBackend for ScriptedVolumeBackend {
    fn create_volume<'a>(
        &'a self,
        _request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error> {
        self.volume_call(VolumeCall::Create, || VolumeHandle {
            id: String::from("vol-123"),
            zone: String::from("test-zone"),
        })
    }

    fn get_volume<'a>(
        &'a self,
        _zone: &'a str,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, VolumeState, Self::Error> {
        self.volume_call(VolumeCall::Get, volume_state)
    }

    fn wait_for_volume_status<'a>(
        &'a self,
        _zone: &'a str,
        _volume_id: &'a str,
        _status: VolumeStatus,
    ) -> BackendFuture<'a, VolumeState, Self::Error> {
        self.volume_call(VolumeCall::WaitReady, volume_state)
    }

    fn delete_volume<'a>(
        &'a self,
        _handle: &'a VolumeHandle,
    ) -> BackendFuture<'a, (), Self::Error> {
        self.volume_call(VolumeCall::Delete, || ())
    }

    fn attach_volume<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        self.volume_call(VolumeCall::Attach, || ())
    }

    fn detach_volume<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        self.volume_call(VolumeCall::Detach, || ())
    }

    fn teardown<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
        _volume_id: Option<&'a str>,
    ) -> BackendFuture<'a, (), TeardownFailure<Self::Error>> {
        Box::pin(async move { self.run_teardown() })
    }
}
