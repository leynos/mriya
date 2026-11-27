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

use camino::Utf8Path;
use cap_std::{
    ambient_authority,
    fs::{File, OpenOptions},
    fs_utf8::Dir,
};
use clap::Parser;
use mriya::bench::{
    billing::load_price_sheet,
    campaign::{CampaignPlan, JsonLinesSink, ScalewayDriver, run_campaign},
    record::OPT_IN_ENV,
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
    let records_text = match read_text(&args.records) {
        Ok(contents) => contents,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    match validate_str(&records_text) {
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
    let records_text = match read_text(&args.records) {
        Ok(contents) => contents,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    let campaign = match validate_str(&records_text) {
        Ok(campaign) => campaign,
        Err(complaints) => {
            report(&format_complaints(&complaints));
            return 1;
        }
    };
    let sheet_text = match read_text(&args.price_sheet) {
        Ok(contents) => contents,
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
    let plan_text = match read_text(&args.campaign) {
        Ok(contents) => contents,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    let plan = match CampaignPlan::from_toml(&plan_text) {
        Ok(plan) => plan,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    // The opt-in is checked before the driver is built, so a refusal never
    // depends on credentials being present. An operator who has not enabled
    // paid runs should not have to configure a provider to be told so.
    let opt_in = paid_run_opt_in(&mockable::DefaultEnv);
    if let Err(err) = mriya::bench::campaign::require_opt_in(opt_in.as_deref()) {
        report(&err.to_string());
        return 1;
    }
    // Checking the destination before the driver means an operator with a
    // stale output file is told so without needing provider credentials.
    //
    // The claim is taken before the check and held through the final flush, so
    // two campaigns with different identifiers cannot both pass the check and
    // write a header each.
    let _claim = match OutputClaim::acquire(&args.out) {
        Ok(claim) => claim,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    if let Err(message) = require_empty(&args.out) {
        report(&message);
        return 1;
    }
    let driver = match ScalewayDriver::from_env() {
        Ok(driver) => driver,
        Err(message) => {
            report(&message);
            return 1;
        }
    };
    // A campaign file begins with exactly one header, so appending a second
    // campaign would produce a file the validator rejects. Never truncate an
    // existing record set.
    let file = match open_append(&args.out) {
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

/// Returns the paid-run opt-in as the operator set it.
///
/// Reading it through [`mockable::Env`] lets both permission outcomes be
/// tested without mutating the process environment; the entry point supplies
/// [`mockable::DefaultEnv`].
fn paid_run_opt_in(env: &impl mockable::Env) -> Option<String> { env.string(OPT_IN_ENV) }

/// Reads a UTF-8 file through a capability handle on its parent directory.
///
/// The binary must be able to read a path the operator names, but it should not
/// reach the rest of the filesystem through the ambient working directory, so
/// the handle is rooted at the file's own parent.
fn read_text(path: &str) -> Result<String, String> {
    let (directory, name) =
        parent_handle(path).map_err(|err| format!("cannot read {path}: {err}"))?;
    directory
        .read_to_string(name)
        .map_err(|err| format!("cannot read {path}: {err}"))
}

/// Rejects an output path that already holds records.
///
/// An absent file is fine, and so is an empty one; anything else would mix two
/// campaigns under two headers in one file.
fn require_empty(path: &str) -> Result<(), String> {
    // A missing parent is reported by the open, which has a better message.
    let Ok((directory, name)) = parent_handle(path) else {
        return Ok(());
    };
    // A missing file is fine. Any other read failure is not: a file whose
    // contents cannot be decoded must not be appended to, because neither the
    // validator nor the report could read the result.
    let existing = match directory.read_to_string(&name) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(format!(
                "cannot inspect {path}: {err}; pass a new path rather than appending to a file \
                 that cannot be read"
            ));
        }
    };
    if existing.trim().is_empty() {
        return Ok(());
    }
    Err(format!(
        "{path} already holds records; pass a new path or move the existing file, because a \
         campaign file must begin with exactly one header"
    ))
}

/// Opens a file for appending, creating it when absent.
fn open_append(path: &str) -> Result<File, String> {
    let (directory, name) = parent_handle(path)?;
    let options = {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        options
    };
    directory
        .as_ref()
        .open_with(&name, &options)
        .map_err(|err| format!("cannot open {path}: {err}"))
}

/// Returns a capability handle on `path`'s parent and the file's own name.
fn parent_handle(path: &str) -> Result<(Dir, String), String> {
    let parsed = Utf8Path::new(path);
    let name = parsed
        .file_name()
        .ok_or_else(|| format!("{path} names no file"))?
        .to_owned();
    let parent = parsed
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .unwrap_or_else(|| Utf8Path::new("."));
    let directory = Dir::open_ambient_dir(parent, ambient_authority())
        .map_err(|err| format!("cannot open {parent}: {err}"))?;
    Ok((directory, name))
}

/// Writes one line to standard output, ignoring a closed stream.
fn say(message: &str) { writeln!(io::stdout(), "{message}").ok(); }

/// Writes one line to standard error, ignoring a closed stream.
fn report(message: &str) { writeln!(io::stderr(), "{message}").ok(); }

/// An exclusive claim on one output path, held for the life of a paid run.
struct OutputClaim {
    /// Directory holding the claim file.
    directory: Dir,
    /// Name of the claim file.
    name: String,
}

impl OutputClaim {
    /// Claims `output` by creating `<output>.lock` exclusively.
    ///
    /// The claim is a separate file so the output itself can be appended to
    /// normally. A leftover claim means a previous run did not finish; the
    /// error says so rather than guessing that it is stale.
    ///
    /// # Errors
    ///
    /// Returns a message when another run holds the claim, or when the claim
    /// file cannot be created.
    fn acquire(output: &str) -> Result<Self, String> {
        let claim_path = format!("{output}.lock");
        let (directory, name) = parent_handle(&claim_path)?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        match directory.as_ref().open_with(&name, &options) {
            Ok(_) => Ok(Self { directory, name }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Err(format!(
                "{output} is claimed by a run that did not finish; move {claim_path} aside and \
                 retry"
            )),
            Err(err) => Err(format!("cannot claim {output}: {err}")),
        }
    }
}

impl Drop for OutputClaim {
    fn drop(&mut self) { self.directory.remove_file(&self.name).ok(); }
}

#[cfg(test)]
mod tests {
    //! Tests for the binary's environment boundary.

    use mockable::MockEnv;

    use super::{OPT_IN_ENV, paid_run_opt_in};

    #[test]
    fn the_opt_in_is_read_through_the_environment_boundary() {
        let mut env = MockEnv::new();
        env.expect_string()
            .returning(|key| (key == OPT_IN_ENV).then(|| String::from("1")));
        assert_eq!(paid_run_opt_in(&env).as_deref(), Some("1"));
    }

    #[test]
    fn an_unset_opt_in_is_reported_as_absent() {
        let mut env = MockEnv::new();
        env.expect_string().returning(|_| None);
        assert_eq!(paid_run_opt_in(&env), None);
    }
}
