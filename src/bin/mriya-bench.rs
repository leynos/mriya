//! Command-line entry point for the backend cost benchmark.
//!
//! Three subcommands, deliberately separated by what they can cost:
//!
//! - `validate` reads a campaign file and reports every defect. It never contacts a provider and is
//!   safe to run anywhere.
//! - `report` prices a validated campaign against a price sheet and prints the comparison. It is
//!   equally offline.
//! - `run` performs a paid campaign. It refuses to start unless the operator has set the opt-in
//!   variable and declared the limits in the campaign file, and it stops at the first cleanup
//!   doubt.
//!
//! The paid path is not reachable from any Make aggregate goal or CI job.

use std::{
    io::{self, Write as _},
    process,
};

use clap::Parser;
use mriya::bench::{
    billing::load_price_sheet,
    campaign::{CampaignPlan, JsonLinesSink, PlannedAttempt, ScalewayDriver, run_campaign},
    record::{CampaignLimits, OPT_IN_ENV},
    report::{build_report, render},
    validate::{format_complaints, validate_str},
};

/// Top-level CLI for the benchmark binary.
#[derive(Debug, Parser)]
#[command(
    name = "mriya-bench",
    about = "Measure backend cost per completed build and test workload",
    arg_required_else_help = true
)]
enum Cli {
    /// Check a campaign file against the benchmark protocol without spending.
    Validate(ValidateArgs),
    /// Price a validated campaign against a dated price sheet.
    Report(ReportArgs),
    /// Run a paid campaign. Refuses unless explicitly enabled by the operator.
    Run(RunArgs),
}

/// Arguments for `mriya-bench validate`.
#[derive(Debug, Parser)]
struct ValidateArgs {
    /// Campaign file to check.
    records: String,
}

/// Arguments for `mriya-bench report`.
#[derive(Debug, Parser)]
struct ReportArgs {
    /// Campaign file to price.
    records: String,
    /// Price sheet to price it with.
    #[arg(long, value_name = "PATH")]
    price_sheet: String,
}

/// Arguments for `mriya-bench run`.
#[derive(Debug, Parser)]
struct RunArgs {
    /// Campaign plan to execute.
    #[arg(long, value_name = "PATH")]
    campaign: String,
    /// Where results are appended, as JSON Lines.
    #[arg(long, value_name = "PATH")]
    out: String,
}

fn main() {
    let exit_code = match Cli::parse() {
        Cli::Validate(args) => exec_validate(&args),
        Cli::Report(args) => exec_report(&args),
        Cli::Run(args) => exec_run(&args),
    };
    process::exit(exit_code);
}

/// Runs `validate`, printing every complaint it finds.
///
/// # Examples
///
/// Running it against a valid campaign prints one line and exits zero, while a
/// defective one prints one line per defect and exits one.
fn exec_validate(args: &ValidateArgs) -> i32 {
    let text = match read_text(&args.records) {
        Ok(text) => text,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    match validate_str(&text) {
        Ok(campaign) => {
            say(&format!(
                "{}: {} runs, {} phases, {} campaign header",
                campaign.id(),
                campaign.runs.len(),
                campaign.phases.len(),
                1
            ));
            0
        }
        Err(complaints) => {
            report(&format_complaints(&complaints));
            1
        }
    }
}

/// Runs `report`, printing the priced comparison.
fn exec_report(args: &ReportArgs) -> i32 {
    let text = match read_text(&args.records) {
        Ok(text) => text,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    let campaign = match validate_str(&text) {
        Ok(campaign) => campaign,
        Err(complaints) => {
            report(&format_complaints(&complaints));
            return 1;
        }
    };
    let sheet_text = match read_text(&args.price_sheet) {
        Ok(text) => text,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    let sheet = match load_price_sheet(&sheet_text) {
        Ok(sheet) => sheet,
        Err(err) => {
            report(&err.to_string());
            return 1;
        }
    };
    match build_report(&campaign, &sheet) {
        Ok(report) => {
            say(&render(&report));
            i32::from(!report.comparability_problems.is_empty())
        }
        Err(err) => {
            report(&err.to_string());
            1
        }
    }
}

/// Runs a paid campaign through the real Scaleway driver.
///
/// The opt-in is read from the process environment and passed explicitly, so
/// the policy is visible at the call site rather than hidden inside the driver.
fn exec_run(args: &RunArgs) -> i32 {
    let text = match read_text(&args.campaign) {
        Ok(text) => text,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    let plan = match parse_plan(&text) {
        Ok(plan) => plan,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    // The opt-in is checked before the driver is built, so a refusal never
    // depends on credentials being present. An operator who has not enabled
    // paid runs should not have to configure a provider to be told so.
    let opt_in = std::env::var(OPT_IN_ENV).ok();
    if let Err(err) = mriya::bench::campaign::require_opt_in(opt_in.as_deref()) {
        report(&err.to_string());
        return 1;
    }
    let driver = match ScalewayDriver::from_env() {
        Ok(driver) => driver,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.out)
    {
        Ok(file) => file,
        Err(err) => {
            report(&format!("cannot open {}: {err}", args.out));
            return 1;
        }
    };
    let mut sink = JsonLinesSink::new(io::BufWriter::new(file));
    match run_campaign(&plan, opt_in.as_deref(), &driver, &mut sink) {
        Ok(()) => {
            say(&format!(
                "campaign {} completed; records appended to {}",
                plan.campaign_id, args.out
            ));
            0
        }
        Err(err) => {
            report(&err.to_string());
            1
        }
    }
}

/// Reads a UTF-8 file, reporting the failing path rather than a bare error.
fn read_text(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))
}

/// Writes one line to standard output, ignoring a closed stream.
fn say(message: &str) { writeln!(io::stdout(), "{message}").ok(); }

/// Writes one line to standard error, ignoring a closed stream.
fn report(message: &str) { writeln!(io::stderr(), "{message}").ok(); }

/// Parses a campaign plan from TOML.
///
/// # Errors
///
/// Returns a message naming the path-independent problem, so a plan that omits
/// a limit is refused before any allocation rather than defaulting.
fn parse_plan(text: &str) -> Result<CampaignPlan, String> {
    #[derive(serde::Deserialize)]
    struct RawPlan {
        campaign: RawCampaign,
        #[serde(default)]
        attempts: Vec<RawAttempt>,
    }

    #[derive(serde::Deserialize)]
    struct RawCampaign {
        id: String,
        #[serde(default)]
        price_sheet: Option<String>,
        #[serde(default)]
        providers: Vec<String>,
        limits: RawLimits,
    }

    #[derive(serde::Deserialize)]
    struct RawLimits {
        max_spend_micros: u64,
        max_attempts: u32,
        max_attempt_nanos: u64,
    }

    #[derive(serde::Deserialize)]
    struct RawAttempt {
        workload_id: String,
        provider: String,
        product: String,
        planned_lifetime_nanos: u64,
        estimated_micros: u64,
    }

    let raw: RawPlan =
        toml::from_str(text).map_err(|err| format!("campaign is not readable: {err}"))?;
    let plan = CampaignPlan {
        campaign_id: raw.campaign.id,
        limits: CampaignLimits {
            max_spend_micros: raw.campaign.limits.max_spend_micros,
            max_attempts: raw.campaign.limits.max_attempts,
            max_attempt_nanos: raw.campaign.limits.max_attempt_nanos,
        },
        price_sheet: raw.campaign.price_sheet,
        providers: raw.campaign.providers,
        attempts: raw
            .attempts
            .into_iter()
            .map(|attempt| PlannedAttempt {
                workload_id: attempt.workload_id,
                provider: attempt.provider,
                product: attempt.product,
                planned_lifetime_nanos: attempt.planned_lifetime_nanos,
                estimated_micros: attempt.estimated_micros,
            })
            .collect(),
    };
    plan.validate().map_err(|err| err.to_string())?;
    Ok(plan)
}
