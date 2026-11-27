//! Running totals and currency formatting for priced benchmarks.
//!
//! These are the pieces of the billing model that do not depend on a provider:
//! a running sum of priced fields, and the fixed-point rendering the report
//! prints. They live apart from the charging rules so that a module which only
//! needs to display a total does not also depend on the loader.

use super::{ChargeError, MICROS_PER_UNIT, charge::CostBreakdown};

/// A running sum of [`CostBreakdown`] fields.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Totals {
    /// Summed chargeable lifetime.
    pub chargeable_nanos: u64,
    /// Summed compute price.
    pub compute_micros: u64,
    /// Summed start charges.
    pub start_micros: u64,
    /// Summed retained storage.
    pub storage_micros: u64,
    /// Summed subscription share.
    pub subscription_micros: u64,
    /// Summed absorbed compute time.
    pub absorbed_nanos: u64,
    /// Summed marginal cost.
    pub marginal_micros: u64,
    /// Summed total-account cost.
    pub total_account_micros: u64,
}

impl CostBreakdown {
    /// Adds this breakdown's fields to `totals`.
    ///
    /// # Parameters
    ///
    /// - `totals`: the running sums to add to.
    ///
    /// # Returns
    ///
    /// `Ok(())` once every field has been added.
    ///
    /// # Errors
    ///
    /// Returns [`ChargeError::Overflow`] when a running total would exceed a
    /// 64-bit count, which means the campaign is larger than the sheet's units
    /// can express rather than that any single gate was mispriced.
    pub fn accumulate(&self, totals: &mut Totals) -> Result<(), ChargeError> {
        // Stage the whole update, so a failure part-way leaves the caller's
        // running totals exactly as they were.
        let staged = Totals {
            chargeable_nanos: checked_add(totals.chargeable_nanos, self.chargeable_nanos)?,
            compute_micros: checked_add(totals.compute_micros, self.compute_micros)?,
            start_micros: checked_add(totals.start_micros, self.start_micros)?,
            storage_micros: checked_add(totals.storage_micros, self.storage_micros)?,
            subscription_micros: checked_add(totals.subscription_micros, self.subscription_micros)?,
            absorbed_nanos: checked_add(totals.absorbed_nanos, self.absorbed_nanos)?,
            marginal_micros: checked_add(totals.marginal_micros, self.marginal_micros)?,
            total_account_micros: checked_add(
                totals.total_account_micros,
                self.total_account_micros,
            )?,
        };
        *totals = staged;
        Ok(())
    }
}
/// Formats a count of micro-units as a currency amount with six decimals.
///
/// # Examples
///
/// ```
/// use mriya::bench::billing::format_currency;
///
/// assert_eq!(format_currency("USD", 24_000), "USD 0.024000");
/// ```
#[must_use]
pub fn format_currency(currency: &str, micros: u64) -> String {
    format!("{currency} {}", super::format_micros(micros))
}

/// Returns whether two figures are the same to the micro-unit.
///
/// The report uses this to say "marginal and total-account cost are equal"
/// rather than printing one number twice under two names.
///
/// # Examples
///
/// ```
/// use mriya::bench::billing::figures_agree;
///
/// assert!(figures_agree(1_000, 1_000));
/// assert!(!figures_agree(1_000, 1_001));
/// ```
#[must_use]
pub const fn figures_agree(left: u64, right: u64) -> bool { left == right }

/// Returns `micros` expressed in whole currency units and its remainder.
///
/// # Examples
///
/// ```
/// use mriya::bench::billing::split_units;
///
/// assert_eq!(split_units(1_500_000), (1, 500_000));
/// ```
#[must_use]
pub fn split_units(micros: u64) -> (u64, u64) {
    (
        micros.checked_div(MICROS_PER_UNIT).unwrap_or(0),
        micros.checked_rem(MICROS_PER_UNIT).unwrap_or(0),
    )
}

/// Adds two counts, reporting overflow rather than wrapping.
pub(super) fn checked_add(left: u64, right: u64) -> Result<u64, ChargeError> {
    left.checked_add(right).ok_or(ChargeError::Overflow)
}
