//! Build script for generating the `mriya` man page.
//!
//! The packaging pipeline expects the man page to be available from the
//! build output directory, so we generate it using clap-mangen here.

use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use clap::CommandFactory;
use clap_mangen::Man;

mod cli {
    //! Shared CLI definitions for manpage generation.
    include!("src/cli_shared.rs");
}

use cli::Cli;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = std::io::stdout();
    stdout.write_all(b"cargo:rerun-if-changed=src/cli.rs\n")?;
    stdout.write_all(b"cargo:rerun-if-changed=src/cli_shared.rs\n")?;

    let out_dir =
        PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "OUT_DIR was not set")
        })?);

    let mut buffer = Vec::new();
    Man::new(Cli::command()).render(&mut buffer)?;

    let mut file = File::create(out_dir.join("mriya.1"))?;
    file.write_all(&buffer)?;

    Ok(())
}
