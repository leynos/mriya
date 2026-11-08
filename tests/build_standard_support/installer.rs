//! The installer half of the build-tools contract. `scripts/install-build-tools.sh` runs in a sandbox
//! whose release server is a local directory and whose `rustup` only logs its arguments, so a case can
//! publish a mold linker archive, tamper with its checksum, or leave a pinned mold linker already on `PATH`, and then
//! read exactly what the installer unpacked and which toolchain and components it asked for.

use std::process::Output;

use super::sandbox::{CARGO_LOG, Sandbox};

/// The pinned release, and the archive name the installer derives from the host's architecture.
const VERSION: &str = "2.41.0";
/// Host utilities the installer uses besides the ones every script needs.
const UTILITIES: &[&str] = &[
    "bash",
    "env",
    "uname",
    "dirname",
    "grep",
    "awk",
    "tr",
    "mktemp",
    "rm",
    "sed",
    "cat",
    "curl",
    "tar",
    "gzip",
    "sha256sum",
    "mkdir",
];

/// How the published archive's checksum relates to its contents.
#[derive(Clone, Copy)]
pub enum Digest {
    /// The checksum file records the archive's real digest.
    Honest,
    /// The checksum file records a digest of different bytes.
    Tampered,
}

/// Returns the archive name for the host.
fn archive_name() -> String {
    format!("mold-{VERSION}-{}-linux.tar.gz", std::env::consts::ARCH)
}

/// Runs a host tool in the sandbox and returns its stdout, trimmed.
fn capture(sandbox: &Sandbox, program: &str, args: &[&str]) -> Result<String, String> {
    let output = sandbox
        .command(program)?
        .args(args)
        .output()
        .map_err(|error| format!("running {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Builds a sandbox with a local release directory holding the pinned archive, a checksum file for it,
/// and a `rustup` that logs its arguments.
///
/// # Errors
///
/// Returns the reason when a host tool is missing or the archive cannot be built.
pub fn published(digest: Digest) -> Result<Sandbox, String> {
    let sandbox = Sandbox::new()?;
    sandbox.link_tools(UTILITIES)?;
    sandbox.write_tool("rustup", "echo \"$*\" >> \"$CARGO_LOG\"")?;
    let name = archive_name();
    let staged = format!("stage/{}", name.trim_end_matches(".tar.gz"));
    sandbox.make_dir(format!("{staged}/bin"))?;
    sandbox.make_dir(format!("release/v{VERSION}"))?;
    sandbox.write_file(format!("{staged}/bin/mold"), b"#!/bin/sh\n")?;
    let archive = format!("release/v{VERSION}/{name}");
    let root = sandbox.root().to_string();
    capture(
        &sandbox,
        "tar",
        &[
            "-czf",
            &format!("{root}/{archive}"),
            "-C",
            &format!("{root}/stage"),
            name.trim_end_matches(".tar.gz"),
        ],
    )?;
    let real = capture(&sandbox, "sha256sum", &[&format!("{root}/{archive}")])?;
    let recorded = match digest {
        Digest::Honest => real
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned(),
        Digest::Tampered => "0".repeat(64),
    };
    sandbox.write_file("SHA256SUMS", format!("{recorded}  {name}\n").as_bytes())?;
    Ok(sandbox)
}

/// Runs the installer in a sandbox, pointed at the sandbox's own release directory and checksum file.
///
/// # Errors
///
/// Returns the reason when the installer cannot be started.
pub fn run_installer(sandbox: &Sandbox) -> Result<Output, String> {
    let root = sandbox.root().to_string();
    sandbox
        .command("bash")?
        .arg("scripts/install-build-tools.sh")
        .env("LINKER_RELEASE_BASE_URL", format!("file://{root}/release"))
        .env("LINKER_SHA256SUMS_FILE", format!("{root}/SHA256SUMS"))
        .output()
        .map_err(|error| format!("running the installer: {error}"))
}

/// Returns what the fake `rustup` was asked, one call per line, or `None` when it never ran.
///
/// # Errors
///
/// Returns the reason when the log exists but cannot be read.
pub fn rustup_calls(sandbox: &Sandbox) -> Result<Option<String>, String> {
    sandbox.read(CARGO_LOG)
}

/// Returns whether the installer unpacked `mold` into the sandbox's prefix.
///
/// # Errors
///
/// Returns the reason when the file exists but cannot be read.
pub fn installed_linker(sandbox: &Sandbox) -> Result<bool, String> {
    Ok(sandbox.read("prefix/bin/mold")?.is_some())
}
