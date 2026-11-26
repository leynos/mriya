//! Provider rate rules and the arithmetic that applies them.
//!
//! Every function here is a pure integer computation over a rule the sheet
//! declares, so it can be tested against the provider's published rule without
//! a loader, a file, or a network.

use super::PriceSheetError;
use crate::bench::billing::{BYTES_PER_GB, MICROS_PER_UNIT, NANOS_PER_HOUR, NANOS_PER_SECOND};

/// The rate a provider charges for one compute product.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductRule {
    /// List price for one hour of the product, in micro-units.
    pub price_per_hour_micros: u64,
    /// Seconds billable time is rounded up to.
    pub granularity_seconds: u64,
    /// Minimum chargeable period for one separately billed resource, in seconds.
    pub minimum_seconds: u64,
    /// Monthly cap per resource, in micro-units, when the provider has one.
    pub monthly_cap_micros: Option<u64>,
    /// One-off charge for creating a resource, in micro-units.
    pub start_charge_micros: u64,
}

/// The rate a provider charges for retained storage.
///
/// Providers quote storage per decimal gigabyte per month, so this rule does
/// too: converting to gibibytes here would introduce a factor that no
/// provider's documentation states, and the resulting price would no longer be
/// checkable against the invoice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageRule {
    /// Price for one decimal gigabyte held for one month, in micro-units.
    pub price_per_gb_month_micros: u64,
    /// Seconds billable time is rounded up to.
    pub granularity_seconds: u64,
    /// Minimum chargeable period for one resource, in seconds.
    pub minimum_seconds: u64,
}

impl ProductRule {
    /// Returns the price of `chargeable_nanos` at this rule's hourly rate.
    ///
    /// The hourly rate is applied by exact integer proportion rather than by a
    /// per-second rate, so a long attempt cannot accumulate a rounding error.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::billing::{NANOS_PER_HOUR, ProductRule};
    ///
    /// let rule = ProductRule {
    ///     price_per_hour_micros: 36_000,
    ///     granularity_seconds: 1,
    ///     minimum_seconds: 0,
    ///     monthly_cap_micros: None,
    ///     start_charge_micros: 0,
    /// };
    /// assert_eq!(rule.price_micros(NANOS_PER_HOUR), Some(36_000));
    /// assert_eq!(rule.price_micros(NANOS_PER_HOUR / 2), Some(18_000));
    /// ```
    #[must_use]
    pub fn price_micros(&self, chargeable_nanos: u64) -> Option<u64> {
        let nanos = u128::from(chargeable_nanos);
        let rate = u128::from(self.price_per_hour_micros);
        let denominator = u128::from(NANOS_PER_HOUR);
        // Round up: a provider that bills for a period bills for all of it.
        let numerator = nanos.checked_mul(rate)?;
        let price = numerator
            .checked_add(denominator.checked_sub(1)?)?
            .checked_div(denominator)?;
        u64::try_from(price).ok()
    }

    /// Returns the price of one resource, capped when the provider caps it.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::billing::{NANOS_PER_HOUR, ProductRule};
    ///
    /// let rule = ProductRule {
    ///     price_per_hour_micros: 1_000,
    ///     granularity_seconds: 1,
    ///     minimum_seconds: 0,
    ///     monthly_cap_micros: Some(1_500),
    ///     start_charge_micros: 0,
    /// };
    /// assert_eq!(rule.capped_price_micros(NANOS_PER_HOUR * 5), Some(1_500));
    /// ```
    #[must_use]
    pub fn capped_price_micros(&self, chargeable_nanos: u64) -> Option<u64> {
        let price = self.price_micros(chargeable_nanos)?;
        Some(self.monthly_cap_micros.map_or(price, |cap| price.min(cap)))
    }
}

impl StorageRule {
    /// Returns the price of holding `bytes` for `chargeable_nanos`.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::billing::{BYTES_PER_GB, NANOS_PER_SECOND, StorageRule};
    ///
    /// let rule = StorageRule {
    ///     price_per_gb_month_micros: 100_000,
    ///     granularity_seconds: 1,
    ///     minimum_seconds: 0,
    /// };
    /// // One gigabyte for a tenth of a 30-day month is a tenth of the price.
    /// let tenth = NANOS_PER_SECOND * 259_200;
    /// assert_eq!(
    ///     rule.price_micros(BYTES_PER_GB, tenth, 2_592_000),
    ///     Some(10_000)
    /// );
    /// ```
    #[must_use]
    pub fn price_micros(
        &self,
        bytes: u64,
        chargeable_nanos: u64,
        month_seconds: u64,
    ) -> Option<u64> {
        let numerator = u128::from(bytes)
            .checked_mul(u128::from(chargeable_nanos))?
            .checked_mul(u128::from(self.price_per_gb_month_micros))?;
        let denominator = u128::from(BYTES_PER_GB)
            .checked_mul(u128::from(month_seconds))?
            .checked_mul(u128::from(NANOS_PER_SECOND))?;
        let price = numerator
            .checked_add(denominator.checked_sub(1)?)?
            .checked_div(denominator)?;
        u64::try_from(price).ok()
    }
}

/// Rounds `nanos` up to whole `granularity_seconds` units and floors the result
/// at `minimum_seconds`.
///
/// # Errors
///
/// Returns [`PriceSheetError::ZeroGranularity`] when the rule declares a
/// granularity of zero, which cannot define a billable unit.
///
/// # Examples
///
/// ```
/// use mriya::bench::billing::{NANOS_PER_SECOND, chargeable_nanos};
///
/// // Two minutes at an hourly granularity bills one whole hour.
/// let billed = chargeable_nanos(120 * NANOS_PER_SECOND, 3_600, 3_600)?;
/// assert_eq!(billed, 3_600 * NANOS_PER_SECOND);
/// # Ok::<(), mriya::bench::billing::PriceSheetError>(())
/// ```
pub fn chargeable_nanos(
    measured_nanos: u64,
    granularity_seconds: u64,
    minimum_seconds: u64,
) -> Result<u64, PriceSheetError> {
    if granularity_seconds == 0 {
        return Err(PriceSheetError::ZeroGranularity);
    }
    let granularity = granularity_seconds.saturating_mul(NANOS_PER_SECOND);
    let units = measured_nanos
        .checked_add(granularity.saturating_sub(1))
        .ok_or(PriceSheetError::Overflow)?;
    let rounded = units
        .checked_div(granularity)
        .and_then(|count| count.checked_mul(granularity))
        .ok_or(PriceSheetError::Overflow)?;
    let minimum = minimum_seconds.saturating_mul(NANOS_PER_SECOND);
    Ok(rounded.max(minimum))
}

/// Converts `micros` from one currency into the sheet's currency.
///
/// # Errors
///
/// Returns [`PriceSheetError::Overflow`] when the converted amount does not fit
/// in a 64-bit count.
///
/// # Examples
///
/// ```
/// use mriya::bench::billing::{PriceSheetError, convert_micros};
///
/// // Two whole units of a currency worth 0.92 each is 1.84.
/// assert_eq!(convert_micros(2_000_000, 920_000)?, 1_840_000);
/// # Ok::<(), PriceSheetError>(())
/// ```
pub fn convert_micros(micros: u64, rate_micros_per_unit: u64) -> Result<u64, PriceSheetError> {
    let numerator = u128::from(micros)
        .checked_mul(u128::from(rate_micros_per_unit))
        .ok_or(PriceSheetError::Overflow)?;
    let denominator = u128::from(MICROS_PER_UNIT);
    let rounding = denominator
        .checked_sub(1)
        .ok_or(PriceSheetError::Overflow)?;
    let converted = numerator
        .checked_add(rounding)
        .ok_or(PriceSheetError::Overflow)?
        .checked_div(denominator)
        .ok_or(PriceSheetError::Overflow)?;
    u64::try_from(converted).map_err(|_| PriceSheetError::Overflow)
}

/// Converts a currency amount in micro-units into a fixed decimal string.
///
/// The report prints money in this form so it never has to use a float.
///
/// # Examples
///
/// ```
/// use mriya::bench::billing::format_micros;
///
/// assert_eq!(format_micros(1_234_567), "1.234567");
/// assert_eq!(format_micros(0), "0.000000");
/// ```
#[must_use]
pub fn format_micros(micros: u64) -> String {
    let whole = micros.checked_div(MICROS_PER_UNIT).unwrap_or(0);
    let fraction = micros.checked_rem(MICROS_PER_UNIT).unwrap_or(0);
    format!("{whole}.{fraction:06}")
}
