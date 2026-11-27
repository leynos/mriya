//! Provider lifecycle events for the benchmark harness.
//!
//! A benchmark record has to name the resources a run created, so a residual
//! record can list what a failed cleanup left behind. That information exists
//! only inside the provider adapter, and the [`Backend`](crate::backend::Backend)
//! port deliberately does not expose it: a port that returned provider
//! identifiers would force every future adapter to invent them.
//!
//! This module is the narrower seam. An adapter may be constructed with a
//! lifecycle sink, and reports each resource it creates and each teardown it
//! completes. A sink is optional and defaults to none, so an ordinary run pays
//! nothing and observes nothing.
//!
//! The distinction between "the create call failed" and "the create call may
//! have succeeded but we could not read the answer" is the one that matters
//! for cleanup: a send or read failure leaves a resource that may exist and is
//! not named in any handle, so it becomes [`LifecycleEvent::AllocationUncertain`]
//! and the harness records the uncertainty rather than assuming nothing was
//! created.

use std::sync::Arc;

/// A resource lifecycle event an adapter reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleEvent {
    /// A resource was created and its identifier is known.
    ResourceCreated {
        /// Provider that owns the resource, for example `scaleway`.
        provider: String,
        /// Provider identifier, such as a server UUID.
        resource_id: String,
        /// Location or zone the resource lives in.
        location: String,
        /// Product or resource class that was created.
        product: String,
    },
    /// A create request may have produced a resource whose identifier is unknown.
    ///
    /// This is reported for a transport failure, a response that could not be
    /// read, and a response that could not be parsed. It is deliberately
    /// pessimistic: a resource that might exist must be looked for.
    AllocationUncertain {
        /// Provider that may hold the resource.
        provider: String,
        /// Location the request targeted.
        location: String,
        /// What made the outcome uncertain, without credentials or payloads.
        reason: String,
    },
    /// A delete request was issued.
    TeardownRequested {
        /// Provider that owns the resource.
        provider: String,
        /// Identifier of the resource being deleted.
        resource_id: String,
    },
    /// A delete request completed and the provider confirmed the resource gone.
    TeardownVerified {
        /// Provider that owned the resource.
        provider: String,
        /// Identifier of the deleted resource.
        resource_id: String,
    },
    /// A delete request completed without confirmation that the resource is gone.
    TeardownUnverified {
        /// Provider that may still hold the resource.
        provider: String,
        /// Identifier of the resource that could not be confirmed gone.
        resource_id: String,
        /// Why confirmation was impossible, without credentials or payloads.
        reason: String,
    },
}

impl LifecycleEvent {
    /// Returns the provider the event concerns.
    #[must_use]
    pub fn provider(&self) -> &str {
        match self {
            Self::ResourceCreated { provider, .. }
            | Self::AllocationUncertain { provider, .. }
            | Self::TeardownRequested { provider, .. }
            | Self::TeardownVerified { provider, .. }
            | Self::TeardownUnverified { provider, .. } => provider,
        }
    }

    /// Returns the resource identifier the event names, when it names one.
    #[must_use]
    pub fn resource_id(&self) -> Option<&str> {
        match self {
            Self::ResourceCreated { resource_id, .. }
            | Self::TeardownRequested { resource_id, .. }
            | Self::TeardownVerified { resource_id, .. }
            | Self::TeardownUnverified { resource_id, .. } => Some(resource_id),
            Self::AllocationUncertain { .. } => None,
        }
    }
}

/// Receives an adapter's lifecycle events.
///
/// Implementations must not panic and must not block: an adapter calls them on
/// its own run path.
pub trait LifecycleSink: Send + Sync {
    /// Records one event.
    ///
    /// # Parameters
    ///
    /// - `event`: the lifecycle event to record. Implementations must not mutate it or retain a
    ///   borrow of it.
    fn record(&self, event: &LifecycleEvent);
}

/// A sink held by an adapter, shared so a harness can read what it collected.
pub type SharedLifecycleSink = Arc<dyn LifecycleSink>;

/// A sink that keeps every event, for tests and for the campaign harness.
#[derive(Debug, Default)]
pub struct RecordingSink {
    /// Events in the order they arrived.
    events: std::sync::Mutex<Vec<LifecycleEvent>>,
}

impl RecordingSink {
    /// Returns a snapshot of the events recorded so far.
    ///
    /// # Returns
    ///
    /// Every event in arrival order. A poisoned lock is recovered rather than
    /// reported, because the events already recorded are still what happened.
    #[must_use]
    pub fn events(&self) -> Vec<LifecycleEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Returns whether the sink saw a create whose outcome was uncertain.
    ///
    /// An uncertain allocation means a resource may exist without any
    /// identifier reaching the harness, which an operator has to reconcile
    /// differently from a known residual.
    ///
    /// # Returns
    ///
    /// `true` when any recorded event reported an uncertain allocation.
    #[must_use]
    pub fn allocation_uncertain(&self) -> bool {
        self.events()
            .iter()
            .any(|event| matches!(event, LifecycleEvent::AllocationUncertain { .. }))
    }

    /// Returns the identifiers of resources the sink saw created.
    ///
    /// # Returns
    ///
    /// The identifiers in creation order. This is a historical record: it
    /// includes resources that were subsequently deleted, so it must not be
    /// read as a list of resources that still exist.
    #[must_use]
    pub fn created_ids(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                LifecycleEvent::ResourceCreated { resource_id, .. } => Some(resource_id),
                _ => None,
            })
            .collect()
    }
}

impl LifecycleSink for RecordingSink {
    fn record(&self, event: &LifecycleEvent) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event.clone());
    }
}

impl<T: LifecycleSink + ?Sized> LifecycleSink for Arc<T> {
    fn record(&self, event: &LifecycleEvent) { (**self).record(event); }
}

impl<T: LifecycleSink + ?Sized> LifecycleSink for Box<T> {
    fn record(&self, event: &LifecycleEvent) { (**self).record(event); }
}

/// Reports `event` to `sink` when one is configured.
///
/// # Parameters
///
/// - `sink`: the configured lifecycle sink, or `None` when the caller was not constructed with one.
///   A `None` sink discards the event without effect.
/// - `event`: the lifecycle event to report.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
///
/// use mriya::bench::lifecycle::{LifecycleEvent, RecordingSink, SharedLifecycleSink, notify};
///
/// let sink = Arc::new(RecordingSink::default());
/// let shared: SharedLifecycleSink = sink.clone();
/// notify(
///     Some(&shared),
///     &LifecycleEvent::ResourceCreated {
///         provider: String::from("scaleway"),
///         resource_id: String::from("srv-1"),
///         location: String::from("fr-par-1"),
///         product: String::from("DEV1-S"),
///     },
/// );
/// assert_eq!(sink.created_ids(), vec![String::from("srv-1")]);
/// ```
pub fn notify(sink: Option<&SharedLifecycleSink>, event: &LifecycleEvent) {
    if let Some(configured) = sink {
        configured.record(event);
    }
}
