//! Phase observation for the run pipeline.
//!
//! The orchestrator reports each phase's span to an observer and never reads
//! anything back from it, so an observer cannot change a run's outcome. These
//! helpers exist so every phase is timed the same way and no phase can be
//! measured twice.

use std::{future::Future, time::Instant};

use super::{RunError, RunOrchestrator};
use crate::{
    backend::{Backend, InstanceHandle},
    bench::{observe::Observation, record::PhaseKind},
    sync::CommandRunner,
};

impl<B, R> RunOrchestrator<B, R>
where
    B: Backend,
    B::Error: std::fmt::Display + Send + Sync + std::error::Error + 'static,
    R: CommandRunner,
{
    /// Awaits `work` and reports it to the observer as one phase.
    ///
    /// The future is created before the phase starts, so a caller's own
    /// arguments are evaluated outside the measurement and only the provider's
    /// work is timed.
    ///
    /// # Examples
    ///
    /// With a recording observer, `observe_async(PhaseKind::Allocate, future)`
    /// yields the future's value and records exactly one finished `Allocate`
    /// phase, whatever the future resolves to.
    pub(super) async fn observe_async<T, E, F>(&self, phase: PhaseKind, work: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        self.observer.observe(Observation::PhaseStarted(phase));
        let started = Instant::now();
        let result = work.await;
        self.observer.observe(Observation::PhaseFinished {
            phase,
            duration: started.elapsed(),
            succeeded: result.is_ok(),
        });
        result
    }

    /// Reports a synchronous phase to the observer.
    ///
    /// The sync path and the cache mount are blocking calls, so they are timed
    /// around the call rather than inside a future.
    pub(super) fn observe_sync<T, E>(
        &self,
        phase: PhaseKind,
        work: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        self.observer.observe(Observation::PhaseStarted(phase));
        let started = Instant::now();
        let result = work();
        self.observer.observe(Observation::PhaseFinished {
            phase,
            duration: started.elapsed(),
            succeeded: result.is_ok(),
        });
        result
    }

    /// Destroys the instance as the final phase.
    ///
    /// Teardown is the phase whose success cannot be assumed: the backend's
    /// `destroy` resolves once the provider has confirmed the resource is
    /// gone, so a resolved call is a verified teardown and a rejected one is
    /// not.
    pub(super) async fn destroy_observed(
        &self,
        handle: InstanceHandle,
    ) -> Result<(), RunError<B::Error>> {
        self.observe_async(PhaseKind::Teardown, self.backend.destroy(handle))
            .await
            .map_err(RunError::Teardown)
    }
}
