//! Tests for reading a price sheet and converting its currencies.

use rstest::rstest;

use super::{
    super::billing::{
        NANOS_PER_HOUR,
        NANOS_PER_SECOND,
        PriceSheetError,
        ProductRule,
        Usage,
        charge,
        format_micros,
        load_price_sheet,
    },
    support::{example_sheet, hourly_storage, sheet_with},
};

#[test]
fn a_zero_exchange_rate_is_refused_rather_than_making_everything_free() {
    let text = r#"
[sheet]
currency = "EUR"
retrieved = "2026-11-26"

[fx.USD]
micros_per_unit = 0
source = "https://example.test/fx"

[providers.x]
currency = "USD"
source = "https://example.test/x"

[providers.x.products.p]
price_per_hour_micros = 36000
granularity_seconds = 1
minimum_seconds = 0
"#;
    let sheet = load_price_sheet(text).expect("the sheet parses");
    let usage = Usage::new("x", "p", NANOS_PER_HOUR);
    let error = charge(&sheet, &usage).expect_err("a zero rate would make the charge free");
    assert!(error.to_string().contains("zero rate"), "{error}");
}

#[test]
fn a_sheet_without_a_currency_is_refused() {
    let error = load_price_sheet("[sheet]\n").expect_err("a unitless price is unusable");
    assert!(matches!(error, PriceSheetError::MissingCurrency));
}

#[test]
fn a_provider_quoting_in_an_undeclared_currency_is_refused() {
    let text = r#"
[sheet]
currency = "EUR"
retrieved = "2026-11-26"

[providers.x]
currency = "USD"
source = "https://example.test/x"

[providers.x.products.p]
price_per_hour_micros = 100
granularity_seconds = 3600
"#;
    let sheet = load_price_sheet(text).expect("the sheet parses");
    let usage = Usage::new("x", "p", NANOS_PER_HOUR);
    let error = charge(&sheet, &usage).expect_err("no rate was declared");
    assert!(error.to_string().contains("USD"), "{error}");
}

#[test]
fn a_declared_conversion_rate_makes_cross_currency_pricing_auditable() {
    let text = r#"
[sheet]
currency = "EUR"
retrieved = "2026-11-26"

[fx.USD]
micros_per_unit = 920000
source = "https://example.test/fx"

[providers.x]
currency = "USD"
source = "https://example.test/x"

[providers.x.products.p]
price_per_hour_micros = 36000
granularity_seconds = 1
minimum_seconds = 0
"#;
    let sheet = load_price_sheet(text).expect("the sheet parses");
    let usage = Usage::new("x", "p", NANOS_PER_HOUR);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    assert_eq!(breakdown.provider_currency, "USD");
    assert_eq!(breakdown.fx_rate_micros, 920_000);
    // USD 0.036 converted at 0.92 EUR per USD is EUR 0.03312.
    assert_eq!(breakdown.marginal_micros, 33_120);
}

#[test]
fn the_example_sheet_records_where_every_rate_came_from() {
    let sheet = example_sheet();
    for (name, provider) in &sheet.providers {
        assert!(
            provider.source.starts_with("https://"),
            "{name} has no source"
        );
    }
    assert_eq!(sheet.currency, "EUR");
    assert_eq!(sheet.retrieved, "2026-11-26");
}

#[test]
fn money_formats_without_a_float() {
    assert_eq!(format_micros(0), "0.000000");
    assert_eq!(format_micros(1), "0.000001");
    assert_eq!(format_micros(1_234_567), "1.234567");
    assert_eq!(format_micros(20_000_000), "20.000000");
}

#[rstest]
#[case(1_000_000_000, 17, "a raw one-second record is rounded to the minimum")]
#[case(
    17_000_000_000,
    17,
    "an already billed 17 seconds is not rounded again"
)]
fn a_billed_lifetime_is_not_rounded_twice(
    #[case] measured_nanos: u64,
    #[case] expected_seconds: u64,
    #[case] why: &str,
) {
    // A four-second granularity with a 17-second minimum cannot express 17 as a
    // whole number of granularity units, so re-rounding it would give 20.
    let product = ProductRule {
        price_per_hour_micros: 3_600_000,
        granularity_seconds: 4,
        minimum_seconds: 17,
        monthly_cap_micros: None,
        start_charge_micros: 0,
    };
    let sheet = sheet_with(product, hourly_storage());
    let mut usage = Usage::new("prov", "p", measured_nanos);
    if measured_nanos > 1_000_000_000 {
        usage = usage.with_billed_lifetime(measured_nanos);
    }
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    assert_eq!(
        breakdown.chargeable_nanos,
        expected_seconds * NANOS_PER_SECOND,
        "{why}"
    );
}

#[test]
fn a_sheet_without_a_retrieval_date_is_refused() {
    let error = load_price_sheet("[sheet]\ncurrency = \"EUR\"\n")
        .expect_err("a rate without a date cannot be re-checked");
    assert!(
        matches!(error, PriceSheetError::MissingRetrievalDate),
        "{error}"
    );
}

#[test]
fn a_rate_without_a_source_is_refused() {
    let text = r#"
[sheet]
currency = "EUR"
retrieved = "2026-11-26"

[providers.x]

[providers.x.products.p]
price_per_hour_micros = 1
granularity_seconds = 1
"#;
    let error = load_price_sheet(text).expect_err("a rate without a source cannot be re-checked");
    assert!(
        matches!(error, PriceSheetError::MissingSource(_)),
        "{error}"
    );
}
