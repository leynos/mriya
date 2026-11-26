//! Shared fixtures for the benchmark tests.
//!
//! The fixtures live beside these tests as real JSON Lines files and a real
//! price sheet, so a test reads the same text an operator's campaign would
//! contain. That keeps them honest: a change to a record's serialized shape
//! breaks the fixtures, and the tests notice, rather than a struct literal
//! silently agreeing with itself.

use std::collections::BTreeMap;

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};

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
    record::{Record, RunRecord},
};

/// Reads a fixture from the benchmark fixtures directory.
///
/// The read goes through a capability handle rooted at the manifest directory,
/// so it cannot follow a path outside the repository even if a fixture name
/// were ever built from something other than a literal.
///
/// # Panics
///
/// Panics when the fixture is missing, because a missing fixture means the test
/// suite itself is broken rather than that the code under test failed.
pub fn fixture(name: &str) -> String {
    let manifest = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = Dir::open_ambient_dir(manifest, ambient_authority())
        .expect("the manifest directory should open");
    directory
        .read_to_string(format!("src/bench/fixtures/{name}"))
        .expect("the fixture should read")
}

/// Returns the example sheet shipped with the repository.
///
/// # Panics
///
/// Panics when the sheet is missing or unreadable, for the reason [`fixture`]
/// gives.
pub fn example_sheet() -> PriceSheet {
    let manifest = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = Dir::open_ambient_dir(manifest, ambient_authority())
        .expect("the manifest directory should open");
    let text = directory
        .read_to_string("benchmarks/price-sheet.example.toml")
        .expect("the example price sheet should read");
    load_price_sheet(&text).expect("the example price sheet should parse")
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

/// A run record that validates clean under `campaign_id`.
pub fn run_record(index: usize) -> RunRecord {
    match super::super::record::parse_line(
        super::fixture("valid-campaign.jsonl")
            .lines()
            .nth(1)
            .expect("the fixture has a run line"),
    )
    .expect("the fixture's run line parses")
    .into_run(index)
    {
        Ok(run) => run,
        Err(err) => panic!("{err}"),
    }
}

/// Converts a parsed record into a run record, numbering its repetition.
trait IntoRun {
    /// Returns the run record, numbering it `index`.
    fn into_run(self, index: usize) -> Result<RunRecord, String>;
}

impl IntoRun for Record {
    fn into_run(self, index: usize) -> Result<RunRecord, String> {
        match self {
            Self::Run(mut run) => {
                run.repetition = u32::try_from(index).unwrap_or(1).saturating_add(1);
                Ok(*run)
            }
            other => Err(format!("expected a run record, got {:?}", other.kind())),
        }
    }
}

/// Returns the repository's example campaign plan.
///
/// # Panics
///
/// Panics when the plan is missing, for the reason [`fixture`] gives.
pub fn example_campaign() -> String {
    let manifest = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = Dir::open_ambient_dir(manifest, ambient_authority())
        .expect("the manifest directory should open");
    directory
        .read_to_string("benchmarks/campaign.example.toml")
        .expect("the example campaign should read")
}
