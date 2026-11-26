//! Integer-only billing arithmetic driven by a source-cited price sheet.
//!
//! A provider does not charge for elapsed seconds. Each one declares its own
//! granularity, its own minimum chargeable period, sometimes a monthly cap, and
//! sometimes a plan whose included usage makes the next gate free. This module
//! turns a measured lifetime into the two figures the protocol requires:
//!
//! - the marginal cost of one more identical gate, assuming the plan and its included usage are
//!   already paid for; and
//! - the total-account cost attributable to the campaign.
//!
//! Every quantity is an integer number of micro-units of currency and of
//! nanoseconds. No floating-point value enters the computation, so two readers
//! of the same record cannot disagree about the price.

mod charge;
mod money;
mod sheet;

pub use charge::{ChargeError, CostBreakdown, RetainedStorage, Usage, charge};
pub use money::{Totals, figures_agree, format_currency, split_units};
pub use sheet::{
    BYTES_PER_GB,
    PlanRule,
    PriceSheet,
    PriceSheetError,
    ProductRule,
    ProviderRules,
    StorageRule,
    chargeable_nanos,
    format_micros,
    load_price_sheet,
};

/// Nanoseconds in one hour, the unit list prices are quoted against.
pub const NANOS_PER_HOUR: u64 = 3_600_000_000_000;

/// Nanoseconds in one second.
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Seconds in a 30-day month, the default period a monthly price covers.
pub const DEFAULT_MONTH_SECONDS: u64 = 2_592_000;

/// Micro-units in one currency unit.
pub const MICROS_PER_UNIT: u64 = 1_000_000;
