//! Tests for the integer-only billing calculator.

use proptest::prelude::*;
use rstest::rstest;

use super::{
    super::billing::{
        BYTES_PER_GB,
        NANOS_PER_HOUR,
        NANOS_PER_SECOND,
        PriceSheetError,
        RetainedStorage,
        Usage,
        charge,
        chargeable_nanos,
        figures_agree,
    },
    support::{example_sheet, hourly_product, hourly_storage, sheet_with, sheet_with_plan},
};

#[rstest]
#[case(0, 1, 0, 0)]
#[case(1, 1, 0, NANOS_PER_SECOND)]
#[case(NANOS_PER_SECOND, 60, 0, 60 * NANOS_PER_SECOND)]
#[case(120 * NANOS_PER_SECOND, 3_600, 3_600, 3_600 * NANOS_PER_SECOND)]
#[case(3_600 * NANOS_PER_SECOND, 3_600, 3_600, 3_600 * NANOS_PER_SECOND)]
#[case(3_601 * NANOS_PER_SECOND, 3_600, 3_600, 7_200 * NANOS_PER_SECOND)]
#[case(1, 3_600, 3_600, 3_600 * NANOS_PER_SECOND)]
fn chargeable_time_rounds_up_and_respects_the_minimum(
    #[case] measured_nanos: u64,
    #[case] granularity_seconds: u64,
    #[case] minimum_seconds: u64,
    #[case] expected_nanos: u64,
) {
    let billed = chargeable_nanos(measured_nanos, granularity_seconds, minimum_seconds)
        .expect("the rule is well formed");
    assert_eq!(billed, expected_nanos);
}

#[test]
fn a_zero_granularity_is_refused_rather_than_dividing_by_zero() {
    assert_eq!(
        chargeable_nanos(NANOS_PER_SECOND, 0, 0),
        Err(PriceSheetError::ZeroGranularity)
    );
}

#[test]
fn scaleway_prices_a_two_minute_gate_as_one_hour() {
    let sheet = example_sheet().expect("the example sheet parses");
    // DEV1-S is EUR 0.00898/hour and bills a 60-minute minimum.
    let usage = Usage::new("scaleway", "DEV1-S", 120 * NANOS_PER_SECOND);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    assert_eq!(breakdown.chargeable_nanos, NANOS_PER_HOUR);
    assert_eq!(breakdown.marginal_micros, 8_980);
    assert!(figures_agree(
        breakdown.marginal_micros,
        breakdown.total_account_micros
    ));
}

#[test]
fn hetzner_caps_a_long_gate_at_the_monthly_price() {
    let sheet = example_sheet().expect("the example sheet parses");
    // CX23 costs EUR 0.0096/hour but never more than EUR 5.99 per month.
    let usage = Usage::new("hetzner", "CX23", 1_000 * NANOS_PER_HOUR);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    assert_eq!(breakdown.marginal_micros, 5_990_000);
}

#[test]
fn boat_bills_per_second_so_a_short_gate_costs_a_fraction_of_an_hour() {
    let sheet = example_sheet().expect("the example sheet parses");
    let usage = Usage::new("boat", "default", 60 * NANOS_PER_SECOND);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    // USD 0.036/hour for one minute is USD 0.0006, and the extra start is
    // USD 0.03; both convert at 0.92 EUR per USD.
    assert_eq!(breakdown.provider_currency, "USD");
    assert_eq!(breakdown.compute_micros, 552);
    assert_eq!(breakdown.start_micros, 27_600);
    assert_eq!(breakdown.marginal_micros, 28_152);
}

#[test]
fn boat_absorbs_a_gate_inside_its_plan_and_reports_the_subscription() {
    let sheet = example_sheet().expect("the example sheet parses");
    let usage = Usage::new("boat", "default", 60 * NANOS_PER_SECOND)
        .with_plan_allowance(NANOS_PER_HOUR)
        .with_starts_used(1)
        .with_subscription_share(20_000_000);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    assert_eq!(breakdown.absorbed_nanos, 60 * NANOS_PER_SECOND);
    assert_eq!(breakdown.compute_micros, 0);
    // The example plan includes no starts, so the start charge still applies:
    // the plan covers time, not starts.
    assert_eq!(breakdown.start_micros, 27_600);
    assert_eq!(breakdown.marginal_micros, 27_600);
    // The plan's USD 20 share converts at 0.92 EUR per USD, and is what makes
    // the total-account figure exceed the marginal one.
    assert_eq!(breakdown.subscription_micros, 18_400_000);
    assert_eq!(
        breakdown.total_account_micros,
        breakdown.marginal_micros.saturating_add(18_400_000)
    );
    assert!(!figures_agree(
        breakdown.marginal_micros,
        breakdown.total_account_micros
    ));
}

#[test]
fn a_plan_that_includes_starts_absorbs_the_start_charge() {
    let sheet = example_sheet().expect("the example sheet parses");
    let usage = Usage::new("boat", "default", 60 * NANOS_PER_SECOND)
        .with_plan_allowance(NANOS_PER_HOUR)
        .with_starts_used(0)
        .with_subscription_share(20_000_000);
    // Boat's plan rule in the example sheet includes no starts, so this asserts
    // the opposite case through a sheet that does.
    let mut patched = sheet;
    if let Some(provider) = patched.providers.get_mut("boat")
        && let Some(plan) = provider.plan.as_mut()
    {
        plan.included_starts = 5;
    }
    let breakdown = charge(&patched, &usage).expect("the rule exists");
    assert_eq!(breakdown.compute_micros, 0);
    assert_eq!(breakdown.start_micros, 0);
    assert_eq!(breakdown.marginal_micros, 0);
    assert_eq!(
        breakdown.total_account_micros, breakdown.subscription_micros,
        "when the plan covers the gate, the bill is the plan's share"
    );
}

#[test]
fn the_subscription_share_is_converted_into_the_sheet_currency() {
    let sheet = example_sheet().expect("the example sheet parses");
    // Boat publishes its plan in USD; the sheet reports in EUR.
    let usage = Usage::new("boat", "default", 0).with_subscription_share(20_000_000);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    assert_eq!(breakdown.provider_currency, "USD");
    assert_eq!(breakdown.subscription_micros, 18_400_000);
}

#[rstest]
#[case(0, 0, "the first start is inside the plan")]
#[case(2, 0, "the last included start is free")]
#[case(3, 10_000, "the first start beyond the plan is charged")]
#[case(4, 10_000, "every later start is charged")]
fn a_start_is_charged_only_beyond_the_plan_allowance(
    #[case] starts_used: u64,
    #[case] expected_micros: u64,
    #[case] why: &str,
) {
    let sheet = sheet_with_plan(hourly_product(), 3);
    let usage = Usage::new("prov", "p", NANOS_PER_HOUR).with_starts_used(starts_used);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    assert_eq!(breakdown.start_micros, expected_micros, "{why}");
}

#[test]
fn boat_charges_its_start_charge_when_no_start_is_included() {
    let sheet = example_sheet().expect("the example sheet parses");
    let usage = Usage::new("boat", "default", 60 * NANOS_PER_SECOND).with_starts_used(0);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    // The example plan includes no starts, so the USD 0.03 start applies and
    // converts at 0.92 EUR per USD.
    assert_eq!(breakdown.start_micros, 27_600);
}

#[test]
fn ubicloud_bills_per_minute() {
    let sheet = example_sheet().expect("the example sheet parses");
    let usage = Usage::new("ubicloud", "standard-2", 61 * NANOS_PER_SECOND);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    assert_eq!(
        breakdown.chargeable_nanos,
        2 * 60 * NANOS_PER_SECOND,
        "61 seconds rounds up to two minutes"
    );
}

#[test]
fn retained_storage_is_priced_over_the_window_it_was_held() {
    let sheet = sheet_with(hourly_product(), hourly_storage());
    let usage = Usage::new("prov", "p", NANOS_PER_HOUR).with_retained(vec![RetainedStorage {
        class: String::from("disk"),
        bytes: 10 * BYTES_PER_GB,
        held_nanos: NANOS_PER_HOUR,
    }]);
    let breakdown = charge(&sheet, &usage).expect("the rule exists");
    // Ten GB at EUR 0.008 per GB-month is EUR 0.08 for a month. One hour is
    // 1/720 of the sheet's 30-day month, so the charge rounds up to the
    // nearest micro-unit: 80,000 / 720 = 111.1, so 112.
    assert_eq!(breakdown.storage_micros, 112);
}

#[test]
fn an_unknown_product_is_named_rather_than_priced_as_zero() {
    let sheet = example_sheet().expect("the example sheet parses");
    let usage = Usage::new("scaleway", "NO-SUCH-PRODUCT", NANOS_PER_HOUR);
    let error = charge(&sheet, &usage).expect_err("the product is absent");
    assert!(
        error.to_string().contains("NO-SUCH-PRODUCT"),
        "the error names the product: {error}"
    );
}

#[test]
fn an_unknown_provider_is_named() {
    let sheet = example_sheet().expect("the example sheet parses");
    let usage = Usage::new("no-such-provider", "p", NANOS_PER_HOUR);
    let error = charge(&sheet, &usage).expect_err("the provider is absent");
    assert!(error.to_string().contains("no-such-provider"), "{error}");
}

proptest! {
    /// An independent oracle recomputes the billable period with division and
    /// a remainder test, rather than the ceiling division the implementation
    /// uses, and the two must agree for every generated rule.
    #[test]
    fn chargeable_time_agrees_with_an_independent_oracle(
        measured_seconds in 0_u64..1_000_000,
        granularity_seconds in 1_u64..7_200,
        minimum_seconds in 0_u64..7_200,
    ) {
        let measured = measured_seconds.saturating_mul(NANOS_PER_SECOND);
        let billed = chargeable_nanos(measured, granularity_seconds, minimum_seconds)
            .expect("granularity is non-zero");
        let granularity = granularity_seconds.saturating_mul(NANOS_PER_SECOND);
        let whole = measured.checked_div(granularity).unwrap_or(0);
        let remainder = measured.checked_rem(granularity).unwrap_or(0);
        let units = if remainder == 0 { whole } else { whole.saturating_add(1) };
        let oracle = units
            .saturating_mul(granularity)
            .max(minimum_seconds.saturating_mul(NANOS_PER_SECOND));
        prop_assert_eq!(billed, oracle);
    }

    /// Billing never charges less than the time measured, and charges either a
    /// whole number of granularity units or exactly the declared minimum.
    ///
    /// The second disjunct is not a loophole. A provider states its
    /// granularity and its minimum period independently, and nothing requires
    /// one to divide the other: a 17-second minimum at a 4-second granularity
    /// bills 17 seconds, which is not a multiple of 4. Requiring a multiple
    /// would force the implementation to invent a rounding rule the provider
    /// never published.
    #[test]
    fn chargeable_time_is_never_below_the_measurement(
        measured_seconds in 0_u64..1_000_000,
        granularity_seconds in 1_u64..7_200,
        minimum_seconds in 0_u64..7_200,
    ) {
        let measured = measured_seconds.saturating_mul(NANOS_PER_SECOND);
        let billed = chargeable_nanos(measured, granularity_seconds, minimum_seconds)
            .expect("granularity is non-zero");
        prop_assert!(billed >= measured);
        let granularity = granularity_seconds.saturating_mul(NANOS_PER_SECOND);
        let minimum = minimum_seconds.saturating_mul(NANOS_PER_SECOND);
        prop_assert!(
            billed.checked_rem(granularity) == Some(0) || billed == minimum,
            "billed {billed} is neither a whole number of {granularity}-nanosecond units nor the \
             {minimum}-nanosecond minimum"
        );
    }

    /// A longer gate never costs less than a shorter one at the same rate.
    #[test]
    fn price_is_monotonic_in_the_measured_lifetime(
        first_seconds in 0_u64..100_000,
        second_seconds in 0_u64..100_000,
    ) {
        let sheet = sheet_with(hourly_product(), hourly_storage());
        let shorter = first_seconds.min(second_seconds);
        let longer = first_seconds.max(second_seconds);
        let cheap = charge(
            &sheet,
            &Usage::new("prov", "p", shorter.saturating_mul(NANOS_PER_SECOND)),
        )
        .expect("the rule exists");
        let dear = charge(
            &sheet,
            &Usage::new("prov", "p", longer.saturating_mul(NANOS_PER_SECOND)),
        )
        .expect("the rule exists");
        prop_assert!(dear.marginal_micros >= cheap.marginal_micros);
    }

    /// The total-account figure is the marginal figure plus the subscription
    /// share, and is therefore never smaller than the marginal figure.
    #[test]
    fn total_account_is_never_below_marginal(
        measured_seconds in 0_u64..100_000,
        subscription_share in 0_u64..100_000_000,
        allowance_seconds in 0_u64..100_000,
    ) {
        let sheet = sheet_with(hourly_product(), hourly_storage());
        let usage = Usage::new("prov", "p", measured_seconds.saturating_mul(NANOS_PER_SECOND))
            .with_plan_allowance(allowance_seconds.saturating_mul(NANOS_PER_SECOND))
            .with_subscription_share(subscription_share);
        let breakdown = charge(&sheet, &usage).expect("the rule exists");
        prop_assert!(breakdown.total_account_micros >= breakdown.marginal_micros);
        prop_assert_eq!(
            breakdown.total_account_micros,
            breakdown.marginal_micros.saturating_add(subscription_share)
        );
    }
}
