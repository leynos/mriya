//! Shared cleanup outcome types reported by every backend.
//!
//! Cleanup is ownership-aware: it removes only resources the ledger recorded
//! as owned, verifies each removal, and preserves retained and borrowed
//! resources in a usable detached state. These types carry that outcome back
//! to the orchestrators without tying them to a provider.

use std::fmt;

use crate::ownership::ResourceRef;

/// A resource that cleanup could not verify removed or preserved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResidualResource {
    /// The resource that remains unverified.
    pub resource: ResourceRef,
    /// Why cleanup could not settle it.
    pub cause: String,
}

impl ResidualResource {
    /// Creates a residual record naming the resource and the cause.
    #[must_use]
    pub fn new(resource: ResourceRef, cause: impl Into<String>) -> Self {
        Self {
            resource,
            cause: cause.into(),
        }
    }
}

impl fmt::Display for ResidualResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.resource, self.cause)
    }
}

/// Successful cleanup outcome.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanupReport {
    /// Owned resources whose absence was verified.
    pub removed: Vec<ResourceRef>,
    /// Retained and borrowed resources verified present and detached.
    pub preserved: Vec<ResourceRef>,
    /// Non-fatal problems, such as a journal that could not be rewritten.
    pub warnings: Vec<String>,
}

impl CleanupReport {
    /// Returns the identifiers of every preserved resource.
    ///
    /// The list is exactly the resources in `preserved`; it does not check
    /// whether an identifier is recorded outside the journal. A caller that
    /// removes the journal must first confirm each retained volume's ID is in
    /// configuration, or recovery can no longer name the volume.
    ///
    /// # Returns
    ///
    /// The provider identifiers of the retained and borrowed resources this
    /// cleanup verified, in the order they were verified.
    #[must_use]
    pub fn preserved_ids(&self) -> Vec<&str> {
        self.preserved
            .iter()
            .map(ResourceRef::id)
            .collect::<Vec<_>>()
    }
}

/// The reasons cleanup could not verify the provider's postconditions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanupFailure {
    /// Resources whose state cleanup could not settle.
    pub residual: Vec<ResidualResource>,
    /// Failures that are not attached to one resource, such as a scope that
    /// could not be listed at all.
    pub causes: Vec<String>,
}

impl CleanupFailure {
    /// Creates an empty failure; callers push residual entries and causes.
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Reports whether no residual resource and no cause was collected.
    #[must_use]
    pub const fn is_empty(&self) -> bool { self.residual.is_empty() && self.causes.is_empty() }

    /// Adds a residual resource.
    pub fn add(&mut self, resource: ResourceRef, cause: impl Into<String>) {
        self.residual.push(ResidualResource::new(resource, cause));
    }

    /// Adds a failure that is not attached to one resource.
    pub fn add_cause(&mut self, cause: impl Into<String>) { self.causes.push(cause.into()); }

    /// Renders the residual resources as `kind id@zone (cause)`.
    fn render_residual(residual: &[ResidualResource]) -> String {
        residual
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

impl fmt::Display for CleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let residual = Self::render_residual(&self.residual);
        let causes = self.causes.join("; ");
        match (residual.is_empty(), causes.is_empty()) {
            (true, true) => formatter.write_str("cleanup failed without a reported cause"),
            (false, true) => write!(formatter, "cleanup could not verify: {residual}"),
            (true, false) => write!(formatter, "cleanup failed: {causes}"),
            (false, false) => write!(formatter, "cleanup could not verify: {residual}; {causes}"),
        }
    }
}

impl std::error::Error for CleanupFailure {}

#[cfg(test)]
mod tests {
    //! Unit tests for residual rendering.

    use rstest::rstest;

    use super::*;

    #[rstest]
    fn residual_renders_kind_id_and_zone_with_cause() {
        let mut failure = CleanupFailure::new();
        failure.add(ResourceRef::server("srv-1", "fr-par-1"), "403 denied");
        failure.add(
            ResourceRef::instance_volume("vol-1", "fr-par-1"),
            "still in_use",
        );
        assert_eq!(
            failure.to_string(),
            "cleanup could not verify: server srv-1@fr-par-1 (403 denied); \
             instance-volume vol-1@fr-par-1 (still in_use)"
        );
    }

    #[rstest]
    fn causes_render_without_residual_resources() {
        let mut failure = CleanupFailure::new();
        failure.add_cause("list servers failed");
        assert_eq!(failure.to_string(), "cleanup failed: list servers failed");
        assert!(!failure.is_empty());
    }
}
