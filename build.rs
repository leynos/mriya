//! Build script for generating the `mriya` man page.
//!
//! The packaging pipeline expects the man page to be available from the
//! build output directory, so we generate it using clap-mangen here.

use std::env;
use std::io::Write;

use camino::Utf8PathBuf;
use cap_std::ambient_authority;
use cap_std::fs_utf8::File;
use clap::CommandFactory;
use clap_mangen::Man;

#[path = "src/cli/mod.rs"]
mod cli;

use cli::Cli;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli/mod.rs");

    let out_dir_os = env::var_os("OUT_DIR")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "OUT_DIR was not set"))?;
    let out_dir =
        Utf8PathBuf::from_path_buf(std::path::PathBuf::from(out_dir_os)).map_err(|path| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("OUT_DIR is not valid UTF-8: {}", path.display()),
            )
        })?;

    let mut buffer = Vec::new();
    Man::new(Cli::command()).render(&mut buffer)?;

    let mut file = File::create_ambient(out_dir.join("mriya.1"), ambient_authority())?;
    file.write_all(&buffer)?;

    Ok(())
}
