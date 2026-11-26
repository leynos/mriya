//! Test doubles for the run orchestrator.
//!
//! The scripted backend records allocations in the run's ledger exactly as the
//! production backend does, then offers three failure shapes the orchestrator
//! must survive: failing after the allocation succeeded, blocking until a
//! cancellation drops the create future, and failing cleanup with residual
//! resource identifiers.

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{
        Arc,
        Mutex,
        PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use mriya::{
    Backend,
    CleanupFailure,
    CleanupReport,
    Disposition,
    InstanceHandle,
    InstanceNetworking,
    InstanceRequest,
    ResourceLedger,
    ResourceRef,
    backend::BackendFuture,
};
use thiserror::Error;

/// Test double for [`Backend`] that records allocations and cleanup attempts.
#[derive(Clone, Debug)]
pub struct ScriptedBackend {
    /// Shared scripted state.
    state: Arc<Mutex<State>>,
    /// Set once create has recorded its allocations.
    recorded: Arc<AtomicBool>,
}

/// Internal mutable state tracked by the scripted backend.
#[derive(Debug, Default)]
struct State {
    /// Whether cleanup reports residual resources.
    fail_cleanup: bool,
    /// Resources cleanup reports as residual.
    residue: Vec<ResourceRef>,
    /// Whether create blocks until its future is dropped.
    block_create: bool,
    /// Whether create fails after recording its allocations.
    fail_after_allocation: bool,
    /// Number of cleanup attempts.
    cleanup_calls: u32,
}

impl ScriptedBackend {
    /// Creates a backend with no failures armed.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            recorded: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Resolves once create has recorded the resources it allocated.
    ///
    /// The flag is polled rather than awaited through a notification type:
    /// `tokio::sync::Notify` holds a `Waker`, whose trait object defeats the
    /// `Any` bound the BDD harness puts on a step's success value.
    pub async fn wait_until_recorded(&self) {
        while !self.recorded.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    }

    /// Makes cleanup fail, reporting `resource` as residual.
    pub fn fail_cleanup(&self, resource: ResourceRef) {
        let mut state = self.lock();
        state.fail_cleanup = true;
        state.residue.push(resource);
    }

    /// Makes create fail after it recorded the resources it allocated.
    pub fn fail_after_allocation(&self) { self.lock().fail_after_allocation = true; }

    /// Makes create record its allocations and then never resolve.
    pub fn block_create_until_cancelled(&self) { self.lock().block_create = true; }

    /// Returns how many cleanup attempts were made.
    pub fn cleanup_calls(&self) -> u32 { self.lock().cleanup_calls }

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

    /// Records the resources a created server owns.
    fn record_allocation(ledger: &ResourceLedger, request: &InstanceRequest) {
        let zone = request.zone.clone();
        ledger.record(
            ResourceRef::server("scripted-id", &zone),
            Disposition::Owned,
        );
        ledger.record(
            ResourceRef::instance_volume("scripted-root", &zone),
            Disposition::Owned,
        );
        ledger.record(
            ResourceRef::public_ip("scripted-ip", &zone, true),
            Disposition::Owned,
        );
        if let Some(volume_id) = request.volume_id.as_deref() {
            ledger.record(
                ResourceRef::instance_volume(volume_id, &zone),
                Disposition::Retained,
            );
        }
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

/// Error variants surfaced by [`ScriptedBackend`].
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ScriptedBackendError {
    /// Raised when the scripted backend is instructed to fail after
    /// allocation.
    #[error("create failure after allocation")]
    CreateAfterAllocation,
}

impl Backend for ScriptedBackend {
    type Error = ScriptedBackendError;

    fn create<'a>(
        &'a self,
        request: &'a InstanceRequest,
        ledger: &'a ResourceLedger,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        Box::pin(async move {
            let (block, fail) = {
                let state = self.lock();
                (state.block_create, state.fail_after_allocation)
            };
            Self::record_allocation(ledger, request);
            self.recorded.store(true, Ordering::Release);
            if fail {
                return Err(ScriptedBackendError::CreateAfterAllocation);
            }
            if block {
                // Dropping this future on cancellation leaves the recorded
                // allocations behind, which is exactly the partial-provisioning
                // case cleanup must handle.
                std::future::pending::<()>().await;
            }
            Ok(InstanceHandle {
                id: String::from("scripted-id"),
                zone: request.zone.clone(),
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

    fn cleanup<'a>(
        &'a self,
        ledger: &'a ResourceLedger,
    ) -> BackendFuture<'a, CleanupReport, CleanupFailure> {
        Box::pin(async move {
            if self.lock().fail_cleanup {
                return Err(self.cleanup_failure());
            }
            self.lock().cleanup_calls += 1;
            Ok(cleanup_report(ledger))
        })
    }
}
