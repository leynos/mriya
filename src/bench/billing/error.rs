//! Why a gate could not be priced.
//!
//! Separate from the arithmetic so both the charging rules and the running
//! totals can name a failure without depending on each other.

use std::fmt;

use super::PriceSheetError;

/// Why a gate could not be priced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChargeError {
    /// A provider, product or storage class named by the record is absent.
    MissingRule(PriceSheetError),
    /// A billing granularity is zero, so no billable unit exists.
    ZeroGranularity,
    /// Arithmetic exceeded a 64-bit count.
    Overflow,
    /// The writer's billed lifetime is shorter than the attempt it describes.
    BilledBelowMeasured,
}

impl fmt::Display for ChargeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRule(source) => write!(formatter, "{source}"),
            Self::ZeroGranularity => {
                formatter.write_str("a price rule declares a billing granularity of zero seconds")
            }
            Self::Overflow => formatter.write_str("price arithmetic exceeded 64-bit range"),
            Self::BilledBelowMeasured => formatter
                .write_str("the recorded billed lifetime is shorter than the measured lifetime"),
        }
    }
}

impl std::error::Error for ChargeError {}

impl From<PriceSheetError> for ChargeError {
    fn from(source: PriceSheetError) -> Self {
        match source {
            PriceSheetError::ZeroGranularity => Self::ZeroGranularity,
            PriceSheetError::Overflow => Self::Overflow,
            other => Self::MissingRule(other),
        }
    }
}
