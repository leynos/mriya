//! Shared fixtures for the benchmark tests.
//!
//! The fixtures live beside these tests as real JSON Lines files and a real
//! price sheet, so a test reads the same text an operator's campaign would
//! contain. That keeps them honest: a change to a record's serialized shape
//! breaks the fixtures, and the tests notice, rather than a struct literal
//! silently agreeing with itself.

use std::collections::BTreeMap;

use super::super::{
    billing::{
        DEFAULT_MONTH_SECONDS,
        PlanRule,
        PriceSheet,
        ProductRule,
        ProviderRules,
        StorageRule,
        load_price_sheet,
    },
    record::{Record, RunRecord, parse_line},
    validate::{Campaign, validate_str},
};

/// The valid campaign fixture, embedded at compile time.
///
/// Embedding it removes a fallible read from shared setup: the fixture is part
/// of the test binary, so a missing or unreadable file is a build failure rather
/// than a panic inside a helper.
pub const VALID_CAMPAIGN: &str = include_str!("../fixtures/valid-campaign.jsonl");

/// The header-only fixture, embedded at compile time.
pub const HEADER_ONLY_CAMPAIGN: &str = include_str!("../fixtures/header-only.jsonl");

/// The defective fixture, embedded at compile time.
pub const DEFECTIVE_CAMPAIGN: &str = include_str!("../fixtures/defective-campaign.jsonl");

/// The repository's example price sheet, embedded at compile time.
pub const EXAMPLE_PRICE_SHEET: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/benchmarks/price-sheet.example.toml"
));

/// The repository's example campaign plan, embedded at compile time.
pub const EXAMPLE_CAMPAIGN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/benchmarks/campaign.example.toml"
));

/// Returns the example sheet shipped with the repository.
///
/// # Errors
///
/// Returns the parse error when the embedded sheet is not valid, which would be
/// a defect in the repository rather than in a test.
pub fn example_sheet() -> Result<PriceSheet, String> {
    load_price_sheet(EXAMPLE_PRICE_SHEET).map_err(|err| err.to_string())
}

/// Returns a fixture by name, for the few tests that need a dynamic choice.
///
/// # Errors
///
/// Returns an error when the name is not one of the embedded fixtures.
pub fn fixture(name: &str) -> Result<&'static str, String> {
    match name {
        "valid-campaign.jsonl" => Ok(VALID_CAMPAIGN),
        "header-only.jsonl" => Ok(HEADER_ONLY_CAMPAIGN),
        "defective-campaign.jsonl" => Ok(DEFECTIVE_CAMPAIGN),
        other => Err(format!("no embedded fixture named {other}")),
    }
}

/// A product that bills a whole hour at EUR 0.036.
pub fn hourly_product() -> ProductRule {
    ProductRule {
        price_per_hour_micros: 36_000,
        granularity_seconds: 3_600,
        minimum_seconds: 3_600,
        monthly_cap_micros: None,
        start_charge_micros: 0,
    }
}

/// A storage class that bills a whole hour at EUR 0.008 per GB-month.
pub fn hourly_storage() -> StorageRule {
    StorageRule {
        price_per_gb_month_micros: 8_000,
        granularity_seconds: 3_600,
        minimum_seconds: 3_600,
    }
}

/// A one-provider sheet with the given product and storage rules.
pub fn sheet_with(product: ProductRule, storage: StorageRule) -> PriceSheet {
    let mut provider_storage = BTreeMap::new();
    provider_storage.insert(String::from("disk"), storage);
    let mut products = BTreeMap::new();
    products.insert(String::from("p"), product);
    let mut providers = BTreeMap::new();
    providers.insert(
        String::from("prov"),
        ProviderRules {
            display_name: String::from("Provider"),
            source: String::from("https://example.test"),
            currency: String::from("EUR"),
            products,
            storage: provider_storage,
            plan: None,
            unpriced_extras: Vec::new(),
        },
    );
    PriceSheet {
        currency: String::from("EUR"),
        retrieved: String::from("2026-11-26"),
        month_seconds: DEFAULT_MONTH_SECONDS,
        providers,
        fx: BTreeMap::new(),
    }
}

/// The same sheet, but with a plan that includes `included_starts` starts and
/// charges EUR 0.01 for each start beyond them.
pub fn sheet_with_plan(product: ProductRule, included_starts: u64) -> PriceSheet {
    let mut sheet = sheet_with(product, hourly_storage());
    let plan = PlanRule {
        name: String::from("fixture plan"),
        monthly_price_micros: 0,
        included_compute_nanos: 0,
        baseline_product: String::from("p"),
        included_starts,
    };
    if let Some(provider) = sheet.providers.get_mut("prov") {
        provider.plan = Some(plan);
        if let Some(rule) = provider.products.get_mut("p") {
            rule.start_charge_micros = 10_000;
        }
    }
    sheet
}

/// Returns the fixture's run record before any test-specific change.
///
/// # Errors
///
/// Returns a message when the embedded fixture has no run line or when that
/// line is not a run record, so a broken fixture is reported rather than
/// replaced by a synthetic record that would agree with itself.
pub fn fixture_run() -> Result<RunRecord, String> {
    let line = VALID_CAMPAIGN
        .lines()
        .nth(1)
        .ok_or_else(|| String::from("the valid campaign fixture has no run line"))?;
    match parse_line(line).map_err(|err| err.to_string())? {
        Record::Run(run) => Ok(*run),
        other => Err(format!("expected a run record, got {:?}", other.kind())),
    }
}

/// A run record that validates clean.
///
/// # Panics
///
/// Panics when the embedded fixture is broken, which is a defect in the test
/// data rather than in the code under test.
pub fn run_record(index: usize) -> RunRecord {
    let mut run = fixture_run().expect("the embedded fixture has a parseable run line");
    run.repetition = u32::try_from(index).unwrap_or(1).saturating_add(1);
    run
}

/// Returns the fixture's header and run lines as a campaign.
pub fn example_campaign_records() -> Result<Campaign, String> {
    validate_str(VALID_CAMPAIGN).map_err(|complaints| format!("{complaints:?}"))
}

/// Builds a campaign from a header and the runs given.
pub fn campaign_with(runs: Vec<RunRecord>) -> Result<Campaign, String> {
    let Some(header_line) = VALID_CAMPAIGN.lines().next().map(str::to_owned) else {
        return Err(String::from("the fixture has no header"));
    };
    let mut text = header_line;
    text.push('\n');
    for (index, mut run) in runs.into_iter().enumerate() {
        // Each attempt needs its own identity, or the validator reports the
        // duplicates this helper would otherwise build.
        run.run_id = format!("{}/run-{index}", run.campaign_id);
        let record = Record::Run(Box::new(run));
        text.push_str(&super::super::record::to_line(&record).map_err(|err| err.to_string())?);
        text.push('\n');
    }
    validate_str(&text).map_err(|complaints| format!("{complaints:?}"))
}

/// Returns the fixture's run record, adjusted by `change`.
///
/// # Panics
///
/// Panics when the embedded fixture is broken, for the reason
/// [`run_record`] gives.
pub fn run_with(change: impl FnOnce(&mut RunRecord)) -> RunRecord {
    let mut run = run_record(0);
    change(&mut run);
    run
}
