//! Cases for the installer half of the build-tools contract: the archive it verifies, the checksums it
//! trusts, the toolchain and components it requests, and the host it skips the linker on. The sandbox
//! turns each of those into a file the case can read back.

use super::{
    build_tools_scripts::expect_actionable_failure,
    config::Pin,
    installer::{
        Digest, SHA256SUMS, archive_name, installed_linker, published, run_installer,
        run_installer_with_toolchain, rustup_calls,
    },
    pins::{TOOLCHAIN, pinned_linker},
};

#[cfg(target_os = "linux")]
use super::installer::run_installer_with_repository_checksums;
#[cfg(target_os = "linux")]
use super::pins::pinned_channel;

/// Scenario: the installer on a host that is not Linux.
///
/// Invariant: it installs the toolchain and skips the linker, even when the published checksum is
/// tampered, because no linker is downloaded at all.
#[test]
fn the_installer_skips_the_linker_on_a_non_linux_host() -> Result<(), String> {
    let sandbox = published(Digest::Tampered)?;
    sandbox.pretend_darwin()?;
    let output = run_installer(&sandbox)?;
    if !output.status.success() {
        return Err(format!(
            "the installer failed on a non-Linux host: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if installed_linker(&sandbox)? {
        return Err("a non-Linux host unpacked a mold linker".to_owned());
    }
    if rustup_calls(&sandbox)?.is_none() {
        return Err("a non-Linux host skipped the toolchain too".to_owned());
    }
    Ok(())
}

/// Scenario: the installer against a published archive, a tampered checksum, and a mold linker already present.
///
/// Invariant: an honest archive is unpacked and the pinned toolchain is requested with every component
/// `rust-toolchain.toml` lists; a checksum mismatch installs nothing and requests no toolchain; a mold linker
/// already at the pin is not downloaded again.
#[cfg(target_os = "linux")]
#[test]
fn the_installer_unpacks_a_verified_archive_and_requests_the_pinned_toolchain() -> Result<(), String>
{
    let sandbox = published(Digest::Honest)?;
    let output = run_installer(&sandbox)?;
    if !output.status.success() {
        return Err(format!(
            "installer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !installed_linker(&sandbox)? {
        return Err("the archive was not unpacked into the prefix".to_owned());
    }
    let calls = rustup_calls(&sandbox)?.unwrap_or_default();
    let expected = format!(
        "toolchain install {} --profile minimal \
         --component rustfmt --component clippy --component rust-analyzer",
        pinned_channel(TOOLCHAIN)?
    );
    if calls.trim() == expected {
        Ok(())
    } else {
        Err(format!("rustup was asked `{calls}`, expected `{expected}`"))
    }
}

#[cfg(target_os = "linux")]
#[test]
fn the_installer_refuses_a_checksum_mismatch_and_installs_nothing() -> Result<(), String> {
    let sandbox = published(Digest::Tampered)?;
    let output = run_installer(&sandbox)?;
    expect_actionable_failure(&output, &["checksum mismatch", "refusing to install"])?;
    if installed_linker(&sandbox)? || rustup_calls(&sandbox)?.is_some() {
        return Err("a refused archive still installed something".to_owned());
    }
    Ok(())
}

/// Scenario: the committed checksum file against a published archive it does not describe.
///
/// Invariant: the installer consults `tools/mold/SHA256SUMS` by default and refuses bytes whose
/// digest it does not record, so the committed file is the one that authorizes an install.
#[cfg(target_os = "linux")]
#[test]
fn the_committed_checksum_refuses_an_archive_it_does_not_describe() -> Result<(), String> {
    let sandbox = published(Digest::Tampered)?;
    let output = run_installer_with_repository_checksums(&sandbox)?;
    expect_actionable_failure(&output, &["checksum mismatch", "refusing to install"])
}

/// Scenario: the committed checksum file itself.
///
/// Invariant: it records exactly one digest for the archive the installer derives for this host,
/// which is the file `make install-build-tools` trusts without a pinned copy of its own.
#[test]
fn the_committed_checksums_record_one_digest_for_the_archive() -> Result<(), String> {
    let name = archive_name();
    let recorded: Vec<&str> = SHA256SUMS
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let digest = fields.next()?;
            (fields.next()? == name).then_some(digest)
        })
        .collect();
    match recorded.as_slice() {
        [_] => Ok(()),
        _ => Err(format!(
            "tools/mold/SHA256SUMS records {recorded:?} for {name}"
        )),
    }
}

/// Scenario: a toolchain file whose components line the installer cannot read.
///
/// Invariant: the installer stops before `rustup` rather than installing a toolchain with no
/// components at all.
#[test]
fn the_installer_stops_before_rustup_when_it_cannot_read_the_components() -> Result<(), String> {
    let sandbox = published(Digest::Honest)?;
    sandbox.write_file(
        "toolchain.toml",
        b"[toolchain]\nchannel = \"nightly\"\ncomponents = rustfmt\n",
    )?;
    let output = run_installer_with_toolchain(&sandbox, "toolchain.toml")?;
    expect_actionable_failure(&output, &["cannot read the components"])?;
    if rustup_calls(&sandbox)?.is_some() {
        return Err("rustup ran although the components could not be read".to_owned());
    }
    Ok(())
}

#[test]
fn the_installer_leaves_a_linker_already_at_the_pin_alone() -> Result<(), String> {
    let sandbox = published(Digest::Tampered)?;
    sandbox.write_tool(
        "mold",
        format!(
            "echo 'mold {pin} (compatible with GNU ld)'",
            pin = pinned_linker()
        ),
    )?;
    let output = run_installer(&sandbox)?;
    if !output.status.success() {
        return Err(format!(
            "installer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // The checksum is tampered, so any download attempt would have failed the run above.
    if installed_linker(&sandbox)? {
        return Err("a mold linker already at the pin was downloaded again".to_owned());
    }
    Ok(())
}

/// Scenario: the toolchain file the installer reads.
///
/// Invariant: `rust-analyzer` joins `rustfmt` and `clippy`, and the pin stays a dated nightly.
#[test]
fn the_toolchain_file_lists_language_server_beside_rustfmt_and_clippy() -> Result<(), String> {
    let components = TOOLCHAIN
        .lines()
        .find(|line| line.trim_start().starts_with("components"))
        .ok_or("rust-toolchain.toml has no components line")?;
    for name in ["rustfmt", "clippy", "rust-analyzer"] {
        if !components.contains(&format!("\"{name}\"")) {
            return Err(format!("{name} is missing from `{components}`"));
        }
    }
    match Pin::read(TOOLCHAIN) {
        Ok(Pin::Nightly) => Ok(()),
        Ok(other) => Err(format!("the pin is {other:?}, not a dated nightly")),
        Err(reason) => Err(reason),
    }
}
