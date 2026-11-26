//! Observation seams for the run pipeline.
//!
//! A benchmark needs to know how long each phase of a run took, and needs that
//! knowledge without changing what a run does. [`RunObserver`] is that seam: the
//! orchestrator reports each phase to an observer and never reads anything back
//! from it, so an observer cannot influence a run's outcome, its return value,
//! or its phase order.
//!
//! [`NoopObserver`] is the default the command-line path uses, and it exists so
//! that "observation is off" is a value rather than a branch. That keeps the
//! existing call sites free of conditionals and keeps the production path
//! identical to what it was before this module existed.
//!
//! The seam is deliberately narrow. It reports durations and outcomes, not
//! provider payloads, so no credential or provider error text can leak into a
//! benchmark record through it.

use std::time::Duration;

use super::record::PhaseKind;

/// An event reported as one phase of a run begins or ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation<'a> {
    /// A phase began.
    PhaseStarted(PhaseKind),
    /// A phase ended.
    PhaseFinished {
        /// Which phase ended.
        phase: PhaseKind,
        /// How long it took.
        duration: Duration,
        /// Whether the phase reached its own success condition.
        succeeded: bool,
    },
    /// A phase finished but its success could not be confirmed.
    ///
    /// The teardown phase uses this when the provider accepted the delete but
    /// did not confirm the resource is gone; nothing else may claim success in
    /// that state.
    PhaseUnverified {
        /// Which phase could not be confirmed.
        phase: PhaseKind,
        /// How long it took.
        duration: Duration,
        /// Why the phase could not be confirmed.
        reason: &'a str,
    },
}

/// Receives the phases of a run.
///
/// Implementations must not panic and must not block: the orchestrator calls
/// them on the run path, so an observer that stalls delays the work it is
/// measuring and corrupts its own measurement.
pub trait RunObserver: Send + Sync {
    /// Records one observation.
    fn observe(&self, observation: Observation<'_>);
}

/// An observer that discards everything, which is what a normal run uses.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use mriya::bench::{
///     observe::{NoopObserver, Observation, RunObserver},
///     record::PhaseKind,
/// };
///
/// let observer = NoopObserver;
/// observer.observe(Observation::PhaseStarted(PhaseKind::Sync));
/// observer.observe(Observation::PhaseFinished {
///     phase: PhaseKind::Sync,
///     duration: Duration::from_secs(1),
///     succeeded: true,
/// });
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoopObserver;

impl RunObserver for NoopObserver {
    fn observe(&self, _observation: Observation<'_>) {}
}

/// Runs `work` and reports the span to `observer` as one phase.
///
/// This is the only place the orchestrator takes a timestamp for observation,
/// so every phase is measured the same way and no phase can be measured twice.
///
/// # Examples
///
/// ```
/// use std::sync::Mutex;
///
/// use mriya::bench::{
///     observe::{Observation, RecordingObserver, time_phase},
///     record::PhaseKind,
/// };
///
/// let observer = RecordingObserver::default();
/// let value = time_phase(&observer, PhaseKind::Allocate, || 7);
/// assert_eq!(value, 7);
/// assert_eq!(observer.phases(), vec![PhaseKind::Allocate]);
/// ```
pub fn time_phase<T, F>(observer: &dyn RunObserver, phase: PhaseKind, work: F) -> T
where
    F: FnOnce() -> T,
{
    observer.observe(Observation::PhaseStarted(phase));
    let started = std::time::Instant::now();
    let value = work();
    observer.observe(Observation::PhaseFinished {
        phase,
        duration: started.elapsed(),
        succeeded: true,
    });
    value
}

/// An observer that keeps the phases it was told about, for tests.
#[derive(Debug, Default)]
pub struct RecordingObserver {
    /// Every observation, in the order it arrived.
    observations: std::sync::Mutex<Vec<RecordedObservation>>,
}

/// One observation as [`RecordingObserver`] stored it, with the duration
/// flattened so a test can compare observations without timing them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordedObservation {
    /// A phase began.
    Started(PhaseKind),
    /// A phase ended.
    Finished {
        /// Which phase ended.
        phase: PhaseKind,
        /// Whether the phase reached its own success condition.
        succeeded: bool,
    },
    /// A phase could not be confirmed.
    Unverified {
        /// Which phase could not be confirmed.
        phase: PhaseKind,
    },
}

impl RecordingObserver {
    /// Returns the phases in the order they were finished, unverified ones
    /// included.
    ///
    /// # Panics
    ///
    /// Panics when a previous holder of the lock panicked. Tests use this to
    /// read their own observations, so a poisoned lock means the test that
    /// poisoned it has already failed.
    #[must_use]
    pub fn phases(&self) -> Vec<PhaseKind> {
        self.observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|observation| match observation {
                RecordedObservation::Started(_) => None,
                RecordedObservation::Finished { phase, .. }
                | RecordedObservation::Unverified { phase } => Some(*phase),
            })
            .collect()
    }

    /// Returns every observation, including the starts.
    ///
    /// # Panics
    ///
    /// Panics when a previous holder of the lock panicked, for the reason
    /// [`RecordingObserver::phases`] gives.
    #[must_use]
    pub fn observations(&self) -> Vec<RecordedObservation> {
        self.observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl RunObserver for RecordingObserver {
    fn observe(&self, observation: Observation<'_>) {
        let recorded = match observation {
            Observation::PhaseStarted(phase) => RecordedObservation::Started(phase),
            Observation::PhaseFinished {
                phase, succeeded, ..
            } => RecordedObservation::Finished { phase, succeeded },
            Observation::PhaseUnverified { phase, .. } => RecordedObservation::Unverified { phase },
        };
        self.observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(recorded);
    }
}
