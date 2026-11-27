//! Ordered teardown stages and the failure that names the stage.

use std::fmt;

/// Stage at which ordered teardown failed.
///
/// The stage tells an orchestrator whether the cache is known to be detached
/// (a destroy failure) or may still be attached (a stop or detach failure).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeardownStage {
    /// The instance could not be stopped.
    Stopping,
    /// The cache could not be detached.
    Detaching,
    /// The server could not be destroyed after detachment.
    Destroying,
}

/// A failed ordered teardown, tagged with the stage that failed.
#[derive(Debug)]
pub struct TeardownFailure<E> {
    /// Stage at which the failure occurred.
    pub stage: TeardownStage,
    /// Underlying backend error.
    pub source: E,
}

impl<E: fmt::Display> fmt::Display for TeardownFailure<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.source)
    }
}

impl<E: std::error::Error + 'static> std::error::Error for TeardownFailure<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { Some(&self.source) }
}

impl TeardownStage {
    /// Returns the stage as a short lower-case phrase.
    ///
    /// # Returns
    ///
    /// `stopping`, `detaching` or `destroying`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopping => "stopping the server",
            Self::Detaching => "detaching the cache",
            Self::Destroying => "destroying the server",
        }
    }
}
