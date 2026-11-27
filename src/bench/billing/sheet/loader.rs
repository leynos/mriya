//! Reading a price sheet from its TOML form.
//!
//! The on-disk shape is deliberately separate from the rules the rest of the
//! code uses: the raw structs can change with the document format without
//! changing [`PriceSheet`], and a parse failure is reported as a parse failure
//! rather than as a missing rule.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{
    FxRule,
    PlanRule,
    PriceSheet,
    PriceSheetError,
    ProductRule,
    ProviderRules,
    StorageRule,
};
use crate::bench::billing::{DEFAULT_MONTH_SECONDS, NANOS_PER_HOUR};

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
    /// Included compute hours per period, in hours of `baseline_product`.
    #[serde(default)]
    included_compute_hours: u64,
    /// The product the included hours are measured in.
    #[serde(default = "default_baseline_product")]
    baseline_product: String,
    /// Resources that start without a start charge.
    #[serde(default)]
    included_starts: u64,
}

/// Returns the product a plan's included hours are measured in by default.
fn default_baseline_product() -> String { String::from("default") }

/// Returns the default month length a sheet uses when it does not say.
const fn default_month_seconds() -> u64 { DEFAULT_MONTH_SECONDS }

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

    let trimmed_retrieved = sheet.retrieved.trim();
    if trimmed_retrieved.is_empty() {
        return Err(PriceSheetError::MissingRetrievalDate);
    }
    let retrieved = trimmed_retrieved.to_owned();
    for (name, provider) in &raw.providers {
        if provider.source.trim().is_empty() {
            return Err(PriceSheetError::MissingSource(format!("provider {name:?}")));
        }
    }
    for (code, rate) in &raw.fx {
        if rate.source.trim().is_empty() {
            return Err(PriceSheetError::MissingSource(format!("[fx.{code}]")));
        }
    }
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
        retrieved,
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
    const fn into_rule(self) -> ProductRule {
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
    const fn into_rule(self) -> StorageRule {
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
            baseline_product: self.baseline_product,
            included_starts: self.included_starts,
        }
    }
}
