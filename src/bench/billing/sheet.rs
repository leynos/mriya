//! The price sheet: provider rates with their sources.
//!
//! A rate without a source is an assertion, not evidence. Every rule in this
//! module therefore carries the URL it was read from and the date it was read,
//! so a reader can re-check it and a campaign can be re-priced later without
//! trusting whoever typed the number.
//!
//! The sheet is TOML. Products and storage classes are keyed by name under a
//! provider, so a record's `provider.product` selects its rule directly and a
//! missing rule is a named error rather than a silent zero.

use std::{collections::BTreeMap, fmt};

use serde::Deserialize;

use super::{MICROS_PER_UNIT, NANOS_PER_HOUR, NANOS_PER_SECOND};

/// A parsed price sheet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceSheet {
    /// Currency every reported figure is expressed in.
    pub currency: String,
    /// Date the rates were read, so a reader knows how stale they are.
    pub retrieved: String,
    /// Seconds the sheet counts as one month, for per-month storage prices.
    pub month_seconds: u64,
    /// Providers keyed by the name a record carries.
    pub providers: BTreeMap<String, ProviderRules>,
    /// Conversion rates from a provider's currency into the sheet's, by code.
    pub fx: BTreeMap<String, FxRule>,
}

impl PriceSheet {
    /// Returns how many micro-units of the sheet currency one micro-unit of
    /// `currency` is worth.
    ///
    /// The sheet's own currency is the identity. Any other currency must be
    /// declared in `fx`, so a comparison between two providers quoting in
    /// different currencies is only possible when somebody has written down
    /// the rate and where it came from.
    ///
    /// # Errors
    ///
    /// Returns [`PriceSheetError::UnknownCurrency`] when the code is neither the
    /// sheet's currency nor a declared rate.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::billing::load_price_sheet;
    ///
    /// let sheet = load_price_sheet(
    ///     r#"
    /// [sheet]
    /// currency = "EUR"
    ///
    /// [fx.USD]
    /// micros_per_unit = 920000
    /// source = "https://example.test/fx"
    /// "#,
    /// )?;
    /// assert_eq!(sheet.rate("EUR")?, 1_000_000);
    /// assert_eq!(sheet.rate("USD")?, 920_000);
    /// # Ok::<(), mriya::bench::billing::PriceSheetError>(())
    /// ```
    pub fn rate(&self, currency: &str) -> Result<u64, PriceSheetError> {
        if currency.eq_ignore_ascii_case(&self.currency) {
            return Ok(MICROS_PER_UNIT);
        }
        self.fx
            .get(currency)
            .map(|rule| rule.micros_per_unit)
            .ok_or_else(|| PriceSheetError::UnknownCurrency(currency.to_owned()))
    }
}

/// A declared conversion rate into the sheet's currency.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FxRule {
    /// Micro-units of the sheet currency per whole unit of this currency.
    pub micros_per_unit: u64,
    /// Where the rate was read from, and ideally when.
    pub source: String,
}

/// One provider's rates and the source they were read from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRules {
    /// Human-readable provider name.
    pub display_name: String,
    /// Where every rate in this provider's entry was read from.
    pub source: String,
    /// Currency this provider's prices are quoted in.
    pub currency: String,
    /// Compute products keyed by the product name a record carries.
    pub products: BTreeMap<String, ProductRule>,
    /// Retained storage classes keyed by name.
    pub storage: BTreeMap<String, StorageRule>,
    /// The subscription that sets a floor under this provider's bill, if any.
    pub plan: Option<PlanRule>,
}

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

/// Bytes in one decimal gigabyte, the unit storage prices are quoted in.
pub const BYTES_PER_GB: u64 = 1_000_000_000;

/// A subscription with included usage, which is what makes marginal and
/// total-account cost differ.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanRule {
    /// Plan name, for the report.
    pub name: String,
    /// Monthly price of the plan, in micro-units.
    pub monthly_price_micros: u64,
    /// Compute time the plan includes per period, in nanoseconds.
    pub included_compute_nanos: u64,
    /// Resources the plan lets start without a start charge, per period.
    pub included_starts: u64,
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
        Some(match self.monthly_cap_micros {
            Some(cap) => price.min(cap),
            None => price,
        })
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
    /// // One gigabyte for a tenth of the sheet's month is a tenth of the price.
    /// let tenth = NANOS_PER_SECOND * 259_200;
    /// assert_eq!(rule.price_micros(BYTES_PER_GB, tenth), Some(10_000));
    /// ```
    #[must_use]
    pub fn price_micros(&self, bytes: u64, chargeable_nanos: u64) -> Option<u64> {
        let numerator = u128::from(bytes)
            .checked_mul(u128::from(chargeable_nanos))?
            .checked_mul(u128::from(self.price_per_gb_month_micros))?;
        let denominator = u128::from(BYTES_PER_GB)
            .checked_mul(u128::from(super::DEFAULT_MONTH_SECONDS))?
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

/// Reasons a price sheet cannot be loaded or used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PriceSheetError {
    /// The sheet text is not valid TOML or does not match the schema.
    Parse(String),
    /// A rule declares a granularity of zero.
    ZeroGranularity,
    /// An arithmetic step exceeded the range of a 64-bit count.
    Overflow,
    /// A record names a provider the sheet does not describe.
    UnknownProvider(String),
    /// A record names a product the provider's entry does not describe.
    UnknownProduct {
        /// Provider named by the record.
        provider: String,
        /// Product named by the record.
        product: String,
    },
    /// A record names a storage class the provider's entry does not describe.
    UnknownStorageClass {
        /// Provider named by the record.
        provider: String,
        /// Storage class named by the record.
        class: String,
    },
    /// The sheet declares no currency, so its prices have no unit.
    MissingCurrency,
    /// A provider quotes in a currency the sheet cannot convert.
    UnknownCurrency(String),
}

impl fmt::Display for PriceSheetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(message) => write!(formatter, "price sheet is not readable: {message}"),
            Self::ZeroGranularity => {
                formatter.write_str("a price rule declares a billing granularity of zero seconds")
            }
            Self::Overflow => formatter.write_str("price arithmetic exceeded 64-bit range"),
            Self::UnknownProvider(name) => {
                write!(formatter, "price sheet has no provider {name:?}")
            }
            Self::UnknownProduct { provider, product } => write!(
                formatter,
                "price sheet has no product {product:?} for provider {provider:?}"
            ),
            Self::UnknownStorageClass { provider, class } => write!(
                formatter,
                "price sheet has no storage class {class:?} for provider {provider:?}"
            ),
            Self::MissingCurrency => {
                formatter.write_str("price sheet declares no currency, so its prices have no unit")
            }
            Self::UnknownCurrency(code) => write!(
                formatter,
                "price sheet cannot convert {code:?}: declare it under [fx.{code}] with \
                 its rate and source"
            ),
        }
    }
}

impl std::error::Error for PriceSheetError {}

/// The on-disk shape of a price sheet, before validation.
///
/// The document has three top-level tables, and the split is deliberate:
/// `[sheet]` holds what the whole document describes, `[providers.<name>]`
/// holds each provider's rates, and `[fx.<CODE>]` holds the conversions that
/// let two providers quoting in different currencies be compared.
#[derive(Debug, Deserialize)]
struct RawDocument {
    /// Sheet-wide metadata.
    sheet: RawSheet,
    /// Providers keyed by name.
    #[serde(default)]
    providers: BTreeMap<String, RawProvider>,
    /// Declared conversion rates into the sheet's currency, keyed by code.
    #[serde(default)]
    fx: BTreeMap<String, RawFx>,
}

/// The sheet-wide metadata table.
#[derive(Debug, Deserialize)]
struct RawSheet {
    /// Currency every reported price is expressed in.
    currency: Option<String>,
    /// Date the rates were read.
    #[serde(default)]
    retrieved: String,
    /// Seconds the sheet counts as a month.
    #[serde(default = "default_month_seconds")]
    month_seconds: u64,
}

/// The on-disk shape of a declared conversion rate.
#[derive(Debug, Deserialize)]
struct RawFx {
    /// Micro-units of the sheet currency per whole unit of this currency.
    micros_per_unit: u64,
    /// Where the rate was read from.
    #[serde(default)]
    source: String,
}

/// The on-disk shape of one provider's entry.
#[derive(Debug, Deserialize)]
struct RawProvider {
    /// Human-readable name.
    #[serde(default)]
    display_name: String,
    /// Where the rates were read from.
    #[serde(default)]
    source: String,
    /// Currency this provider quotes in; the sheet currency when absent.
    #[serde(default)]
    currency: Option<String>,
    /// Compute products keyed by name.
    #[serde(default)]
    products: BTreeMap<String, RawProduct>,
    /// Storage classes keyed by name.
    #[serde(default)]
    storage: BTreeMap<String, RawStorage>,
    /// The subscription, when the provider has one.
    #[serde(default)]
    plan: Option<RawPlan>,
}

/// The on-disk shape of one compute product.
#[derive(Debug, Deserialize)]
struct RawProduct {
    /// List price per hour, in micro-units.
    price_per_hour_micros: u64,
    /// Seconds billable time rounds up to.
    granularity_seconds: u64,
    /// Minimum chargeable seconds per resource.
    #[serde(default)]
    minimum_seconds: u64,
    /// Monthly cap per resource, in micro-units.
    #[serde(default)]
    monthly_cap_micros: Option<u64>,
    /// One-off start charge, in micro-units.
    #[serde(default)]
    start_charge_micros: u64,
}

/// The on-disk shape of one storage class.
#[derive(Debug, Deserialize)]
struct RawStorage {
    /// Price per decimal gigabyte per month, in micro-units.
    price_per_gb_month_micros: u64,
    /// Seconds billable time rounds up to.
    granularity_seconds: u64,
    /// Minimum chargeable seconds per resource.
    #[serde(default)]
    minimum_seconds: u64,
}

/// The on-disk shape of a subscription.
#[derive(Debug, Deserialize)]
struct RawPlan {
    /// Plan name.
    #[serde(default)]
    name: String,
    /// Monthly price, in micro-units.
    #[serde(default)]
    monthly_price_micros: u64,
    /// Included compute hours per period.
    #[serde(default)]
    included_compute_hours: u64,
    /// Resources that start without a start charge.
    #[serde(default)]
    included_starts: u64,
}

/// Returns the default month length a sheet uses when it does not say.
fn default_month_seconds() -> u64 { super::DEFAULT_MONTH_SECONDS }

/// Loads a price sheet from TOML text.
///
/// # Errors
///
/// Returns [`PriceSheetError::Parse`] when the text is not a price sheet, and
/// [`PriceSheetError::MissingCurrency`] when it declares no currency, because a
/// price without a unit cannot be compared with anything.
///
/// # Examples
///
/// ```
/// use mriya::bench::billing::load_price_sheet;
///
/// let sheet = load_price_sheet(
///     r#"
/// [sheet]
/// currency = "USD"
/// retrieved = "2026-11-26"
///
/// [providers.example]
/// source = "https://example.test/pricing"
///
/// [providers.example.products.small]
/// price_per_hour_micros = 36000
/// granularity_seconds = 3600
/// "#,
/// )?;
/// assert_eq!(sheet.currency, "USD");
/// assert!(sheet.providers["example"].products.contains_key("small"));
/// # Ok::<(), mriya::bench::billing::PriceSheetError>(())
/// ```
pub fn load_price_sheet(text: &str) -> Result<PriceSheet, PriceSheetError> {
    let raw: RawDocument =
        toml::from_str(text).map_err(|err| PriceSheetError::Parse(err.to_string()))?;
    let sheet = raw.sheet;
    let currency = sheet
        .currency
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(PriceSheetError::MissingCurrency)?
        .to_owned();

    let providers = raw
        .providers
        .into_iter()
        .map(|(name, provider)| (name, provider.into_rules(&currency)))
        .collect();
    let fx = raw
        .fx
        .into_iter()
        .map(|(code, rule)| {
            (
                code,
                FxRule {
                    micros_per_unit: rule.micros_per_unit,
                    source: rule.source,
                },
            )
        })
        .collect();

    Ok(PriceSheet {
        currency,
        retrieved: sheet.retrieved,
        month_seconds: sheet.month_seconds,
        providers,
        fx,
    })
}

impl RawProvider {
    /// Converts the parsed entry into validated rules.
    fn into_rules(self, sheet_currency: &str) -> ProviderRules {
        let currency = self
            .currency
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(sheet_currency)
            .to_owned();
        ProviderRules {
            display_name: self.display_name,
            source: self.source,
            currency,
            products: self
                .products
                .into_iter()
                .map(|(name, product)| (name, product.into_rule()))
                .collect(),
            storage: self
                .storage
                .into_iter()
                .map(|(name, storage)| (name, storage.into_rule()))
                .collect(),
            plan: self.plan.map(RawPlan::into_rule),
        }
    }
}

impl RawProduct {
    /// Converts the parsed product into a rule.
    fn into_rule(self) -> ProductRule {
        ProductRule {
            price_per_hour_micros: self.price_per_hour_micros,
            granularity_seconds: self.granularity_seconds,
            minimum_seconds: self.minimum_seconds,
            monthly_cap_micros: self.monthly_cap_micros,
            start_charge_micros: self.start_charge_micros,
        }
    }
}

impl RawStorage {
    /// Converts the parsed storage class into a rule.
    fn into_rule(self) -> StorageRule {
        StorageRule {
            price_per_gb_month_micros: self.price_per_gb_month_micros,
            granularity_seconds: self.granularity_seconds,
            minimum_seconds: self.minimum_seconds,
        }
    }
}

impl RawPlan {
    /// Converts the parsed plan, expanding its hours into nanoseconds.
    fn into_rule(self) -> PlanRule {
        PlanRule {
            name: self.name,
            monthly_price_micros: self.monthly_price_micros,
            included_compute_nanos: self.included_compute_hours.saturating_mul(NANOS_PER_HOUR),
            included_starts: self.included_starts,
        }
    }
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
