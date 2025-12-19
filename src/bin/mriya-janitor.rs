//! Scaleway test-run janitor for Mriya.
//!
//! This binary deletes any Scaleway resources tagged with
//! `mriya-test-run-<MRIYA_TEST_RUN_ID>` and then verifies the set is empty.

use clap::Parser;
use mriya::janitor::{DEFAULT_SCW_BIN, Janitor, JanitorConfig, TEST_RUN_ID_ENV};
use std::io::Write as _;

#[derive(Debug, Parser)]
#[command(
    name = "mriya-janitor",
    about = "Delete Scaleway test resources for a single test run"
)]
struct Cli {
    /// Scaleway project id used to scope discovery.
    #[arg(long, env = "SCW_DEFAULT_PROJECT_ID")]
    project_id: String,
    /// Test run id used to compute the tag (`mriya-test-run-<id>`).
    #[arg(long, env = TEST_RUN_ID_ENV)]
    test_run_id: String,
    /// Path to the Scaleway CLI binary.
    #[arg(long, default_value = DEFAULT_SCW_BIN)]
    scw_bin: String,
}

fn main() -> Result<(), String> {
    let cli = Cli::parse();
    let config = JanitorConfig::new(cli.project_id, cli.test_run_id, cli.scw_bin)
        .map_err(|err| err.to_string())?;
    let janitor = Janitor::with_process_runner(config);
    let summary = janitor.sweep().map_err(|err| err.to_string())?;
    writeln!(
        std::io::stdout(),
        "janitor sweep complete: deleted_servers={}, deleted_volumes={}",
        summary.deleted_servers,
        summary.deleted_volumes
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}
