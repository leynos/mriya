//! The installer half of the build-tools contract. `scripts/install-build-tools.sh` runs in a sandbox
//! whose release server is a local directory and whose `rustup` only logs its arguments, so a case can
//! publish a mold linker archive, tamper with its checksum, or leave a pinned mold linker already on `PATH`, and then
//! read exactly what the installer unpacked and which toolchain and components it asked for.

use std::process::{Command, Output};

use super::pins::pinned_linker;
use super::sandbox::{CARGO_LOG, Sandbox};

/// `tools/mold/SHA256SUMS`, the file the installer trusts by default.
pub const SHA256SUMS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tools/mold/SHA256SUMS"
));

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

/// Returns the archive name the installer derives for the host.
pub fn archive_name() -> String {
    format!(
        "mold-{}-{}-linux.tar.gz",
        pinned_linker(),
        std::env::consts::ARCH
    )
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
    let version = pinned_linker();
    let name = archive_name();
    let staged = format!("stage/{}", name.trim_end_matches(".tar.gz"));
    sandbox.make_dir(format!("{staged}/bin"))?;
    sandbox.make_dir(format!("release/v{version}"))?;
    sandbox.write_file(format!("{staged}/bin/mold"), b"#!/bin/sh\n")?;
    let archive = format!("release/v{version}/{name}");
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
    installer_command(sandbox)?
        .env(
            "LINKER_SHA256SUMS_FILE",
            format!("{}/SHA256SUMS", sandbox.root()),
        )
        .output()
        .map_err(|error| format!("running the installer: {error}"))
}

/// Runs the installer with the repository's own `tools/mold/SHA256SUMS`, so a case can prove the
/// committed checksum refuses an archive it does not describe.
///
/// # Errors
///
/// Returns the reason when the installer cannot be started.
pub fn run_installer_with_repository_checksums(sandbox: &Sandbox) -> Result<Output, String> {
    installer_command(sandbox)?
        .output()
        .map_err(|error| format!("running the installer: {error}"))
}

/// Runs the installer with `RUST_TOOLCHAIN_FILE` pointed at `relative`, so a case can hand it a
/// malformed toolchain file.
///
/// # Errors
///
/// Returns the reason when the installer cannot be started.
pub fn run_installer_with_toolchain(sandbox: &Sandbox, relative: &str) -> Result<Output, String> {
    let toolchain = sandbox.root().join(relative);
    installer_command(sandbox)?
        .env(
            "LINKER_SHA256SUMS_FILE",
            format!("{}/SHA256SUMS", sandbox.root()),
        )
        .env("RUST_TOOLCHAIN_FILE", toolchain)
        .output()
        .map_err(|error| format!("running the installer: {error}"))
}

/// Returns the installer command pointed at the sandbox's release directory.
fn installer_command(sandbox: &Sandbox) -> Result<Command, String> {
    let root = sandbox.root().to_string();
    let mut command = sandbox.command("bash")?;
    command
        .arg("scripts/install-build-tools.sh")
        .env("LINKER_RELEASE_BASE_URL", format!("file://{root}/release"));
    Ok(command)
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
