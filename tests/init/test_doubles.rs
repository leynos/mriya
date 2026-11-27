//! Test doubles for init workflow scenarios.
//!
//! The scripted backend records allocations in the init ledger exactly as the
//! production backend does, and cleanup reports what it removed and preserved.
//! The in-memory configuration store can fail a write, which models the case
//! where the new volume must survive for recovery.

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex, PoisonError},
};

use camino::Utf8PathBuf;
use mriya::{
    Backend,
    CleanupFailure,
    CleanupReport,
    ConfigStoreError,
    ConfigWriter,
    Disposition,
    InstanceHandle,
    InstanceNetworking,
    InstanceRequest,
    ResourceLedger,
    ResourceRef,
    VolumeBackend,
    VolumeHandle,
    VolumeRequest,
    backend::BackendFuture,
};
use thiserror::Error;

/// Scripted backend that simulates volume and instance lifecycle operations.
#[derive(Clone, Debug)]
pub struct ScriptedVolumeBackend {
    /// Shared scripted state.
    state: Arc<Mutex<State>>,
}

/// Failure points scripted by a scenario.
#[derive(Clone, Copy, Debug)]
enum FailureMode {
    CreateVolume,
    Provision,
    Wait,
    Cleanup,
}

impl FailureMode {
    const fn flag(self) -> u8 {
        match self {
            Self::CreateVolume => 0b0001,
            Self::Provision => 0b0010,
            Self::Wait => 0b0100,
            Self::Cleanup => 0b1000,
        }
    }
}

/// Bit set of scripted failures.
#[derive(Clone, Copy, Debug, Default)]
struct Failures(u8);

impl Failures {
    const fn set(&mut self, mode: FailureMode) { self.0 |= mode.flag(); }

    const fn contains(self, mode: FailureMode) -> bool { self.0 & mode.flag() != 0 }
}

/// Mutable state tracked by the scripted backend.
#[derive(Debug, Default)]
struct State {
    /// Armed failure points.
    failures: Failures,
    /// Number of volume-creation attempts.
    create_volume_calls: u32,
    /// Number of cleanup attempts.
    cleanup_calls: u32,
    /// Resources cleanup reports as residual when it fails.
    residue: Vec<ResourceRef>,
    /// Resources a successful cleanup reported as removed.
    removed: Vec<ResourceRef>,
    /// Resources a successful cleanup reported as preserved.
    preserved: Vec<ResourceRef>,
}

impl ScriptedVolumeBackend {
    /// Creates a backend with no failures armed.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Makes volume creation fail before any allocation.
    pub fn fail_create_volume(&self) { self.fail(FailureMode::CreateVolume); }

    /// Makes helper-instance creation fail.
    pub fn fail_provision(&self) { self.fail(FailureMode::Provision); }

    /// Makes readiness polling fail.
    pub fn fail_wait(&self) { self.fail(FailureMode::Wait); }

    /// Makes cleanup fail, reporting the helper server as residual.
    pub fn fail_cleanup(&self) {
        let mut state = self.lock();
        state.failures.set(FailureMode::Cleanup);
        state
            .residue
            .push(ResourceRef::server("instance-123", "test-zone"));
    }

    /// Returns how many volume-creation attempts were made.
    pub fn create_volume_calls(&self) -> u32 { self.lock().create_volume_calls }

    /// Returns how many cleanup attempts were made.
    pub fn cleanup_calls(&self) -> u32 { self.lock().cleanup_calls }

    /// Returns the identifiers a successful cleanup reported as removed.
    pub fn removed_ids(&self) -> Vec<String> {
        self.lock()
            .removed
            .iter()
            .map(|resource| resource.id().to_owned())
            .collect()
    }

    /// Returns the identifiers a successful cleanup reported as preserved.
    pub fn preserved_ids(&self) -> Vec<String> {
        self.lock()
            .preserved
            .iter()
            .map(|resource| resource.id().to_owned())
            .collect()
    }

    /// Locks the state, recovering a poisoned lock.
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Builds the failure cleanup reports when it is scripted to fail.
    fn cleanup_failure(&self) -> CleanupFailure {
        let mut state = self.lock();
        state.cleanup_calls += 1;
        let mut failure = CleanupFailure::new();
        for resource in state.residue.clone() {
            failure.add(resource, "403 denied");
        }
        failure
    }

    /// Arms `mode`.
    fn fail(&self, mode: FailureMode) { self.lock().failures.set(mode); }

    /// Reports whether `mode` is armed.
    fn has(&self, mode: FailureMode) -> bool { self.lock().failures.contains(mode) }
}

/// Errors raised by the scripted backend to model failure points.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScriptedVolumeBackendError {
    /// Volume creation failed.
    #[error("volume create failure")]
    VolumeCreate,
    /// Instance provisioning failed.
    #[error("provision failure")]
    Provision,
    /// Readiness polling failed.
    #[error("wait failure")]
    Wait,
}

impl Backend for ScriptedVolumeBackend {
    type Error = ScriptedVolumeBackendError;

    fn create<'a>(
        &'a self,
        request: &'a InstanceRequest,
        ledger: &'a ResourceLedger,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        Box::pin(async move {
            if self.has(FailureMode::Provision) {
                return Err(ScriptedVolumeBackendError::Provision);
            }
            let zone = request.zone.clone();
            ledger.record(
                ResourceRef::server("instance-123", &zone),
                Disposition::Owned,
            );
            ledger.record(
                ResourceRef::instance_volume("vol-root", &zone),
                Disposition::Owned,
            );
            Ok(InstanceHandle {
                id: String::from("instance-123"),
                zone,
            })
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        _handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        Box::pin(async move {
            if self.has(FailureMode::Wait) {
                return Err(ScriptedVolumeBackendError::Wait);
            }
            Ok(InstanceNetworking {
                public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                ssh_port: 22,
            })
        })
    }

    fn cleanup<'a>(
        &'a self,
        ledger: &'a ResourceLedger,
    ) -> BackendFuture<'a, CleanupReport, CleanupFailure> {
        Box::pin(async move {
            if self.has(FailureMode::Cleanup) {
                return Err(self.cleanup_failure());
            }
            let report = cleanup_report(ledger);
            let mut state = self.lock();
            state.cleanup_calls += 1;
            state.removed = report.removed.clone();
            state.preserved = report.preserved.clone();
            drop(state);
            Ok(report)
        })
    }
}

impl VolumeBackend for ScriptedVolumeBackend {
    fn create_volume<'a>(
        &'a self,
        request: &'a VolumeRequest,
        ledger: &'a ResourceLedger,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error> {
        Box::pin(async move {
            self.lock().create_volume_calls += 1;
            if self.has(FailureMode::CreateVolume) {
                return Err(ScriptedVolumeBackendError::VolumeCreate);
            }
            ledger.record(
                ResourceRef::instance_volume("vol-123", &request.zone),
                Disposition::Owned,
            );
            Ok(VolumeHandle {
                id: String::from("vol-123"),
                zone: request.zone.clone(),
            })
        })
    }
}

/// Builds the report a successful cleanup returns from `ledger`.
fn cleanup_report(ledger: &ResourceLedger) -> CleanupReport {
    let mut report = CleanupReport::default();
    for entry in ledger.entries() {
        if entry.disposition.is_deletable() {
            report.removed.push(entry.resource);
        } else {
            report.preserved.push(entry.resource);
        }
    }
    report
}

/// In-memory config store for behavioural tests.
#[derive(Clone, Debug)]
pub struct MemoryConfigStore {
    /// Shared configuration state.
    state: Arc<Mutex<ConfigState>>,
}

/// Mutable configuration state.
#[derive(Debug, Default)]
struct ConfigState {
    /// Identifier already present in configuration.
    existing_volume_id: Option<String>,
    /// Number of successful writes.
    write_calls: u32,
    /// Whether the next write fails.
    fail_write: bool,
}

impl MemoryConfigStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ConfigState::default())),
        }
    }

    /// Creates a store that already holds `volume_id`.
    pub fn with_existing(volume_id: &str) -> Self {
        let store = Self::new();
        store.lock().existing_volume_id = Some(volume_id.to_owned());
        store
    }

    /// Makes the next configuration write fail.
    pub fn fail_write(&self) { self.lock().fail_write = true; }

    /// Returns how many successful writes were made.
    pub fn write_calls(&self) -> u32 { self.lock().write_calls }

    /// Locks the state, recovering a poisoned lock.
    fn lock(&self) -> std::sync::MutexGuard<'_, ConfigState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns the state or an I/O error when the lock is poisoned.
    fn try_lock(&self) -> Result<std::sync::MutexGuard<'_, ConfigState>, ConfigStoreError> {
        self.state.lock().map_err(|err| ConfigStoreError::Io {
            path: Utf8PathBuf::from("in-memory"),
            message: format!("lock poisoned: {err}"),
        })
    }
}

impl ConfigWriter for MemoryConfigStore {
    fn current_volume_id(&self) -> Result<Option<String>, ConfigStoreError> {
        Ok(self.try_lock()?.existing_volume_id.clone())
    }

    fn write_volume_id(
        &self,
        volume_id: &str,
        force: bool,
    ) -> Result<Utf8PathBuf, ConfigStoreError> {
        let mut state = self.try_lock()?;
        if let Some(existing) = state.existing_volume_id.as_ref()
            && !force
        {
            return Err(ConfigStoreError::VolumeAlreadyConfigured {
                volume_id: existing.clone(),
            });
        }
        if state.fail_write {
            return Err(ConfigStoreError::Io {
                path: Utf8PathBuf::from("mriya.toml"),
                message: String::from("simulated write failure"),
            });
        }
        state.existing_volume_id = Some(volume_id.to_owned());
        state.write_calls += 1;
        Ok(Utf8PathBuf::from("mriya.toml"))
    }
}
