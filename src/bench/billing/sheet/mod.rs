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

use super::MICROS_PER_UNIT;

mod loader;
mod pricing;

pub use loader::load_price_sheet;
pub use pricing::{ProductRule, StorageRule, chargeable_nanos, convert_micros, format_micros};

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
