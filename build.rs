//! Build script for generating the `mriya` man page.
//!
//! The packaging pipeline expects the man page to be available from the
//! build output directory, so we generate it using clap-mangen here.

use std::env;
use std::io::Write;
use std::io::{self, ErrorKind};

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
use cap_std::fs_utf8::DirEntry;
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

    cleanup_duplicate_manpages(&out_dir)?;

    Ok(())
}

fn cleanup_duplicate_manpages(out_dir: &Utf8Path) -> io::Result<()> {
    let build_paths = resolve_build_paths(&out_dir.to_path_buf())?;
    let build_root_dir = Dir::open_ambient_dir(&build_paths.build_root, ambient_authority())?;

    for entry_result in build_root_dir.read_dir(".")? {
        let entry = entry_result?;
        process_build_entry(&entry, &build_paths.current_build_name)?;
    }
    Ok(())
}

struct BuildPaths {
    build_root: Utf8PathBuf,
    current_build_name: String,
}

fn resolve_build_paths(out_dir: &Utf8PathBuf) -> io::Result<BuildPaths> {
    let current_build_dir = out_dir.parent().ok_or_else(|| {
        io::Error::new(
            ErrorKind::NotFound,
            "OUT_DIR does not have a parent build directory",
        )
    })?;
    let build_root = current_build_dir.parent().ok_or_else(|| {
        io::Error::new(
            ErrorKind::NotFound,
            "OUT_DIR does not have a build root directory",
        )
    })?;
    let current_build_name = current_build_dir.file_name().ok_or_else(|| {
        io::Error::new(ErrorKind::NotFound, "build directory does not have a name")
    })?;

    Ok(BuildPaths {
        build_root: build_root.to_path_buf(),
        current_build_name: current_build_name.to_owned(),
    })
}

fn process_build_entry(entry: &DirEntry, current_build_name: &str) -> io::Result<()> {
    let entry_name = entry.file_name()?;
    if !entry_name.starts_with("mriya-") || entry_name == current_build_name {
        return Ok(());
    }

    let entry_dir = entry.open_dir()?;
    if let Some(entry_out_dir) = open_optional_dir(&entry_dir, "out")? {
        remove_duplicate_manpage(&entry_out_dir)?;
    }
    Ok(())
}

fn open_optional_dir(dir: &Dir, path: &str) -> io::Result<Option<Dir>> {
    match dir.open_dir(path) {
        Ok(opened_dir) => Ok(Some(opened_dir)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn remove_duplicate_manpage(dir: &Dir) -> io::Result<()> {
    match dir.remove_file("mriya.1") {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}
