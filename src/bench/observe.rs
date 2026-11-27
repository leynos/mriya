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
/// # Parameters
///
/// - `observer`: receives the start and the finish of the phase.
/// - `phase`: which phase is being measured.
/// - `work`: the blocking work to time.
///
/// # Returns
///
/// Whatever `work` returned, unchanged.
///
/// A callback that returns normally is recorded with `succeeded: true`, even
/// when the value it returned is a `Result::Err`. The observation reports
/// whether the phase reached its own success condition, which is a decision
/// only the caller can make; a caller whose `Err` means the phase failed must
/// report that separately rather than relying on this helper.
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

/// One observation as [`RecordingObserver`] stored it.
///
/// The duration is kept because a campaign persists it: an observer that threw
/// the measurement away would leave the harness with a phase name and no
/// number, which is not a measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordedObservation {
    /// A phase began.
    Started(PhaseKind),
    /// A phase ended.
    Finished {
        /// Which phase ended.
        phase: PhaseKind,
        /// How long it took, in nanoseconds.
        duration_nanos: u64,
        /// Whether the phase reached its own success condition.
        succeeded: bool,
    },
    /// A phase could not be confirmed.
    Unverified {
        /// Which phase could not be confirmed.
        phase: PhaseKind,
        /// How long it took, in nanoseconds.
        duration_nanos: u64,
    },
}

impl RecordedObservation {
    /// Returns the phase the observation concerns, or `None` for a start.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::{observe::RecordedObservation, record::PhaseKind};
    ///
    /// let finished = RecordedObservation::Finished {
    ///     phase: PhaseKind::Sync,
    ///     duration_nanos: 1,
    ///     succeeded: true,
    /// };
    /// assert_eq!(finished.phase(), Some(PhaseKind::Sync));
    /// assert_eq!(finished.duration_nanos(), Some(1));
    /// ```
    #[must_use]
    pub const fn phase(&self) -> Option<PhaseKind> {
        match self {
            Self::Started(_) => None,
            Self::Finished { phase, .. } | Self::Unverified { phase, .. } => Some(*phase),
        }
    }

    /// Returns the measured duration, or `None` for a start.
    #[must_use]
    pub const fn duration_nanos(&self) -> Option<u64> {
        match self {
            Self::Started(_) => None,
            Self::Finished { duration_nanos, .. } | Self::Unverified { duration_nanos, .. } => {
                Some(*duration_nanos)
            }
        }
    }

    /// Returns whether the observation reports a successful finish.
    ///
    /// A start is not a success, and an unverified finish is not one either.
    #[must_use]
    pub const fn succeeded(&self) -> bool {
        matches!(
            self,
            Self::Finished {
                succeeded: true,
                ..
            }
        )
    }
}

impl RecordingObserver {
    /// Returns the phases in the order they were finished, unverified ones
    /// included.
    ///
    /// # Returns
    ///
    /// One entry per finished or unverified observation, in arrival order. A
    /// poisoned lock is recovered rather than reported, because the recorded
    /// observations are still the ones the run produced.
    #[must_use]
    pub fn phases(&self) -> Vec<PhaseKind> {
        self.observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(RecordedObservation::phase)
            .collect()
    }

    /// Returns every observation, including the starts.
    ///
    /// # Returns
    ///
    /// A snapshot of every observation, in arrival order.
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
                phase,
                duration,
                succeeded,
            } => RecordedObservation::Finished {
                phase,
                duration_nanos: u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX),
                succeeded,
            },
            Observation::PhaseUnverified {
                phase, duration, ..
            } => RecordedObservation::Unverified {
                phase,
                duration_nanos: u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX),
            },
        };
        self.observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(recorded);
    }
}
