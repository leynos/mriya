//! Why a gate could not be priced.
//!
//! Separate from the arithmetic so both the charging rules and the running
//! totals can name a failure without depending on each other.

use super::PriceSheetError;

/// Why a gate could not be priced.
#[derive(Debug, thiserror::Error)]
pub enum ChargeError {
    /// A provider, product or storage class named by the record is absent.
    #[error("{0}")]
    MissingRule(#[source] PriceSheetError),
    /// A billing granularity is zero, so no billable unit exists.
    #[error("a price rule declares a billing granularity of zero seconds")]
    ZeroGranularity,
    /// Arithmetic exceeded a 64-bit count.
    #[error("price arithmetic exceeded 64-bit range")]
    Overflow,
    /// The writer's billed lifetime is shorter than the attempt it describes.
    #[error("the recorded billed lifetime is shorter than the measured lifetime")]
    BilledBelowMeasured,
}

impl From<PriceSheetError> for ChargeError {
    fn from(source: PriceSheetError) -> Self {
        match source {
            PriceSheetError::ZeroGranularity => Self::ZeroGranularity,
            PriceSheetError::Overflow => Self::Overflow,
            other => Self::MissingRule(other),
        }
    }
}
