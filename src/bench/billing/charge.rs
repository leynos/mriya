//! Turning one measured gate into money.
//!
//! The split between marginal and total-account cost is the whole point of this
//! module. A provider with a monthly plan charges nothing extra for a gate that
//! fits inside the plan's included hours, so the marginal cost of that gate is
//! zero even though the plan costs money every month. Reporting a single figure
//! would hide one of those facts, so [`CostBreakdown`] carries both and states
//! how much compute time the plan absorbed.
//!
//! The subscription is attributed per gate by the caller, in
//! [`Usage::subscription_share_micros`], rather than assumed here. A caller that
//! spans several gates divides the plan price across them; a caller reporting a
//! single gate passes the whole price. That keeps the arithmetic local and
//! auditable.

use std::fmt;

use super::{
    PlanRule,
    PriceSheet,
    PriceSheetError,
    ProductRule,
    ProviderRules,
    StorageRule,
    chargeable_nanos,
    money::checked_add,
};

/// What one gate consumed on one provider.
///
/// Provider-currency amounts are converted into the sheet's currency before
/// they are summed, so a report never adds two different currencies together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Usage {
    /// Provider name, selecting the rules to price against.
    pub provider: String,
    /// Product name within that provider.
    pub product: String,
    /// Measured lifetime, before the provider's granularity and minimum apply.
    pub measured_nanos: u64,
    /// Compute time the plan's included allowance had left before this gate.
    ///
    /// Zero when the provider has no plan, or when the allowance is exhausted.
    pub plan_allowance_remaining_nanos: u64,
    /// Resources started under the plan's included start allowance before this.
    pub plan_starts_used: u64,
    /// Caller-computed share of the plan's monthly price, in micro-units of the
    /// provider's own currency.
    ///
    /// It is quoted that way because the plan's price is what the provider
    /// publishes; [`charge`] converts it into the sheet's currency along with
    /// every other provider-currency amount, so the caller never has to know
    /// the rate.
    pub subscription_share_micros: u64,
    /// Storage held for this gate's campaign window.
    pub retained: Vec<RetainedStorage>,
}

impl Usage {
    /// Starts a usage description for one product.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::billing::Usage;
    ///
    /// let usage = Usage::new("scaleway", "DEV1-S", 3_600_000_000_000);
    /// assert_eq!(usage.plan_allowance_remaining_nanos, 0);
    /// assert!(usage.retained.is_empty());
    /// ```
    #[must_use]
    pub fn new(
        provider: impl Into<String>,
        product: impl Into<String>,
        measured_nanos: u64,
    ) -> Self {
        Self {
            provider: provider.into(),
            product: product.into(),
            measured_nanos,
            plan_allowance_remaining_nanos: 0,
            plan_starts_used: 0,
            subscription_share_micros: 0,
            retained: Vec::new(),
        }
    }

    /// Declares how much of the plan's included compute the gate may absorb.
    #[must_use]
    pub const fn with_plan_allowance(mut self, remaining_nanos: u64) -> Self {
        self.plan_allowance_remaining_nanos = remaining_nanos;
        self
    }

    /// Declares how many included starts had already been used.
    #[must_use]
    pub const fn with_starts_used(mut self, starts_used: u64) -> Self {
        self.plan_starts_used = starts_used;
        self
    }

    /// Declares this gate's amortized share of the plan's monthly price.
    #[must_use]
    pub const fn with_subscription_share(mut self, share_micros: u64) -> Self {
        self.subscription_share_micros = share_micros;
        self
    }

    /// Declares a retained-storage charge held for this gate's window.
    #[must_use]
    pub fn with_retained(mut self, retained: Vec<RetainedStorage>) -> Self {
        self.retained = retained;
        self
    }
}

/// Storage held beyond the gate itself, priced over the window it was held.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedStorage {
    /// Storage class name within the provider.
    pub class: String,
    /// Bytes held.
    pub bytes: u64,
    /// How long the bytes were held, in nanoseconds.
    pub held_nanos: u64,
}

/// The priced result for one gate.
///
/// Every money field is in the sheet's currency, already converted from the
/// provider's own when the two differ. `provider_currency` names the currency
/// the provider quoted in and `fx_rate_micros` the rate applied, so a reader can
/// re-derive the figure from the provider's published price.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CostBreakdown {
    /// Billable lifetime after the provider's granularity and minimum.
    pub chargeable_nanos: u64,
    /// Currency the provider's rates are quoted in.
    pub provider_currency: String,
    /// Micro-units of sheet currency per unit of the provider's currency.
    pub fx_rate_micros: u64,
    /// Price of compute time beyond what the plan absorbed.
    pub compute_micros: u64,
    /// Start charges beyond the plan's included starts.
    pub start_micros: u64,
    /// Retained storage attributable to this gate.
    pub storage_micros: u64,
    /// This gate's share of the plan's monthly price.
    pub subscription_micros: u64,
    /// Compute time the plan absorbed, which costs nothing more.
    pub absorbed_nanos: u64,
    /// What one more identical gate costs, given the plan is already paid.
    pub marginal_micros: u64,
    /// The whole bill attributable to this gate, subscription included.
    pub total_account_micros: u64,
}

/// Why a gate could not be priced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChargeError {
    /// A provider, product or storage class named by the record is absent.
    MissingRule(PriceSheetError),
    /// A billing granularity is zero, so no billable unit exists.
    ZeroGranularity,
    /// Arithmetic exceeded a 64-bit count.
    Overflow,
}

impl fmt::Display for ChargeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRule(source) => write!(formatter, "{source}"),
            Self::ZeroGranularity => {
                formatter.write_str("a price rule declares a billing granularity of zero seconds")
            }
            Self::Overflow => formatter.write_str("price arithmetic exceeded 64-bit range"),
        }
    }
}

impl std::error::Error for ChargeError {}

impl From<PriceSheetError> for ChargeError {
    fn from(source: PriceSheetError) -> Self {
        match source {
            PriceSheetError::ZeroGranularity => Self::ZeroGranularity,
            other => Self::MissingRule(other),
        }
    }
}

/// Prices one gate against a sheet.
///
/// The gate's compute time is charged only for the part the plan does not
/// absorb. A start charge applies only to a resource started beyond the plan's
/// included starts. The subscription share the caller supplied is added to the
/// total-account figure and to nothing else, which is what makes the two
/// figures differ exactly when a platform charges for a plan.
///
/// # Errors
///
/// Returns [`ChargeError`] when the sheet lacks the rule the usage names, when a
/// rule declares a zero granularity, or when the arithmetic overflows.
///
/// # Examples
///
/// ```
/// use mriya::bench::billing::{Usage, charge, load_price_sheet};
///
/// let sheet = load_price_sheet(
///     r#"
/// [sheet]
/// currency = "USD"
///
/// [providers.scaleway]
/// source = "https://www.scaleway.com/en/docs/billing/faq/"
///
/// [providers.scaleway.products.DEV1-S]
/// price_per_hour_micros = 24000
/// granularity_seconds = 3600
/// minimum_seconds = 3600
/// "#,
/// )?;
///
/// // Two minutes of a product billed by the hour costs one whole hour.
/// let usage = Usage::new("scaleway", "DEV1-S", 120_000_000_000);
/// let breakdown = charge(&sheet, &usage)?;
/// assert_eq!(breakdown.marginal_micros, 24_000);
/// assert_eq!(breakdown.marginal_micros, breakdown.total_account_micros);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn charge(sheet: &PriceSheet, usage: &Usage) -> Result<CostBreakdown, ChargeError> {
    let provider = rules_for(sheet, &usage.provider)?;
    let product = product_for(provider, usage)?;
    let rate = sheet.rate(&provider.currency)?;
    let chargeable = chargeable_nanos(
        usage.measured_nanos,
        product.granularity_seconds,
        product.minimum_seconds,
    )?;
    let absorbed_nanos = chargeable.min(usage.plan_allowance_remaining_nanos);
    let billable_nanos = chargeable.saturating_sub(absorbed_nanos);

    let compute_micros = convert(price_compute(product, billable_nanos)?, rate)?;
    let start_micros = convert(start_charge(provider, product, usage), rate)?;
    let storage_micros = convert(
        storage_charge(provider, &usage.retained, sheet.month_seconds)?,
        rate,
    )?;
    let subscription_micros = convert(usage.subscription_share_micros, rate)?;

    let marginal_micros = checked_add(checked_add(compute_micros, start_micros)?, storage_micros)?;
    let total_account_micros = checked_add(marginal_micros, subscription_micros)?;

    Ok(CostBreakdown {
        chargeable_nanos: chargeable,
        provider_currency: provider.currency.clone(),
        fx_rate_micros: rate,
        compute_micros,
        start_micros,
        storage_micros,
        subscription_micros,
        absorbed_nanos,
        marginal_micros,
        total_account_micros,
    })
}

/// Converts a provider-currency amount into the sheet's currency.
fn convert(micros: u64, rate: u64) -> Result<u64, ChargeError> {
    Ok(super::sheet::convert_micros(micros, rate)?)
}

/// Prices `billable_nanos` of compute, reporting overflow rather than wrapping.
fn price_compute(product: &ProductRule, billable_nanos: u64) -> Result<u64, ChargeError> {
    product
        .capped_price_micros(billable_nanos)
        .ok_or(ChargeError::Overflow)
}

/// Selects a provider's rules, or names the provider the sheet lacks.
fn rules_for<'a>(sheet: &'a PriceSheet, provider: &str) -> Result<&'a ProviderRules, ChargeError> {
    sheet.providers.get(provider).ok_or_else(|| {
        ChargeError::MissingRule(PriceSheetError::UnknownProvider(provider.to_owned()))
    })
}

/// Selects a product's rule, or names the product the provider lacks.
fn product_for<'a>(
    provider: &'a ProviderRules,
    usage: &Usage,
) -> Result<&'a ProductRule, ChargeError> {
    provider.products.get(&usage.product).ok_or_else(|| {
        ChargeError::MissingRule(PriceSheetError::UnknownProduct {
            provider: usage.provider.clone(),
            product: usage.product.clone(),
        })
    })
}

/// Returns the start charge owed once the plan's included starts are used up.
fn start_charge(provider: &ProviderRules, product: &ProductRule, usage: &Usage) -> u64 {
    let included = provider
        .plan
        .as_ref()
        .map_or(0, |plan: &PlanRule| plan.included_starts);
    if usage.plan_starts_used < included {
        return 0;
    }
    product.start_charge_micros
}

/// Sums the retained-storage charges, naming any class the sheet lacks.
fn storage_charge(
    provider: &ProviderRules,
    retained: &[RetainedStorage],
    month_seconds: u64,
) -> Result<u64, ChargeError> {
    let mut total = 0_u64;
    for item in retained {
        let rule = storage_rule(provider, item)?;
        let chargeable = chargeable_nanos(
            item.held_nanos,
            rule.granularity_seconds,
            rule.minimum_seconds,
        )?;
        let price = rule
            .price_micros(item.bytes, chargeable, month_seconds)
            .ok_or(ChargeError::Overflow)?;
        total = checked_add(total, price)?;
    }
    Ok(total)
}

/// Selects a storage class's rule, or names the class the provider lacks.
fn storage_rule<'a>(
    provider: &'a ProviderRules,
    item: &RetainedStorage,
) -> Result<&'a StorageRule, ChargeError> {
    provider.storage.get(&item.class).ok_or_else(|| {
        ChargeError::MissingRule(PriceSheetError::UnknownStorageClass {
            provider: provider.display_name.clone(),
            class: item.class.clone(),
        })
    })
}
