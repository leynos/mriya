//! Cases for the installer half of the build-tools contract: the archive it verifies, the checksums it
//! trusts, the toolchain and components it requests, and the host it skips the linker on. The sandbox
//! turns each of those into a file the case can read back.

use rstest::rstest;

use super::{
    build_tools_scripts::expect_actionable_failure,
    config::Pin,
    installer::{
        Digest, SHA256SUMS, archive_name, installed_linker, published, run_installer,
        run_installer_with_toolchain, rustup_calls,
    },
    pins::{TOOLCHAIN, pinned_channel, pinned_components, pinned_linker},
    sandbox::{LINKER_ENTRY, LINKER_TOOL, Sandbox},
};

#[cfg(target_os = "linux")]
use super::installer::run_installer_with_repository_checksums;

/// Returns the one `rustup` request the installer must make for the pinned toolchain.
fn expected_toolchain_request() -> Result<String, String> {
    let components: Vec<String> = pinned_components(TOOLCHAIN)?
        .into_iter()
        .map(|component| format!("--component {component}"))
        .collect();
    Ok(format!(
        "toolchain install {} --profile minimal {}",
        pinned_channel(TOOLCHAIN)?,
        components.join(" ")
    ))
}

/// Runs the installer and checks that it succeeded, left the linker as `linker_expected`, and
/// asked `rustup` for exactly the pinned toolchain and components.
fn expect_the_pinned_toolchain_only(
    sandbox: &Sandbox,
    linker_expected: bool,
    host: &str,
) -> Result<(), String> {
    let output = run_installer(sandbox)?;
    if !output.status.success() {
        return Err(format!(
            "the installer failed on {host}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if installed_linker(sandbox)? != linker_expected {
        let verdict = if linker_expected {
            "was not unpacked"
        } else {
            "was unpacked"
        };
        return Err(format!("{host}: the linker {verdict}"));
    }
    let calls = rustup_calls(sandbox)?.unwrap_or_default();
    let expected = expected_toolchain_request()?;
    if calls.trim() == expected {
        Ok(())
    } else {
        Err(format!(
            "{host} asked rustup `{calls}`, expected `{expected}`"
        ))
    }
}

/// Scenario: the installer on a host that is not Linux.
///
/// Invariant: it requests the pinned toolchain with every component `rust-toolchain.toml` lists and
/// skips the linker, even when the published checksum is tampered, because no linker is downloaded
/// at all.
#[test]
fn the_installer_skips_the_linker_on_a_non_linux_host() -> Result<(), String> {
    let sandbox = published(Digest::Tampered)?;
    sandbox.pretend_darwin()?;
    expect_the_pinned_toolchain_only(&sandbox, false, "a non-Linux host")
}

/// Scenario: the installer on a host whose `uname -s` fails.
///
/// Invariant: an unanswered platform aborts before either half runs. Reading it
/// as "not Linux" would skip the linker and still succeed, so the failure would
/// look like a host that had nothing to install.
#[test]
fn the_installer_stops_when_the_platform_cannot_be_detected() -> Result<(), String> {
    let sandbox = published(Digest::Honest)?;
    sandbox.pretend_uname_fails()?;
    let output = run_installer(&sandbox)?;
    expect_actionable_failure(&output, &["could not detect the host platform"])?;
    if rustup_calls(&sandbox)?.is_some() {
        return Err("the installer went on to install a toolchain".to_owned());
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
    expect_the_pinned_toolchain_only(&sandbox, true, "a Linux host")
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
        LINKER_TOOL,
        format!(
            "echo '{LINKER_TOOL} {pin} (compatible with GNU ld)'",
            pin = pinned_linker()
        ),
    )?;
    // The skip applies to an installed linker, which the compiler reaches as
    // `-fuse-ld=mold`; without this entry point the installer must download.
    sandbox.write_tool(LINKER_ENTRY, "exit 0")?;
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
    let components = pinned_components(TOOLCHAIN)?;
    for name in ["rustfmt", "clippy", "rust-analyzer"] {
        if !components.contains(&name) {
            return Err(format!("{name} is missing from {components:?}"));
        }
    }
    match Pin::read(TOOLCHAIN) {
        Ok(Pin::Nightly) => Ok(()),
        Ok(other) => Err(format!("the pin is {other:?}, not a dated nightly")),
        Err(reason) => Err(reason),
    }
}

/// Scenario: the component list a toolchain file declares.
///
/// Invariant: a bracketed list of quoted names is read in order, with or without a comment or a
/// trailing comma, while a missing, repeated, unquoted or empty declaration is an error rather than
/// an empty list, so the installer contract never asserts against components that were dropped.
#[rstest]
#[case::listed(
    "[toolchain]\ncomponents = [\"rustfmt\", \"clippy\"]\n",
    Some(&["rustfmt", "clippy"][..])
)]
#[case::commented_and_trailing(
    "[toolchain]\ncomponents = [\"rustfmt\",] # pinned\n",
    Some(&["rustfmt"][..])
)]
#[case::another_table("[dependencies]\ncomponents = [\"x\"]\n", None)]
#[case::missing("[toolchain]\nchannel = \"nightly\"\n", None)]
#[case::repeated("[toolchain]\ncomponents = [\"a\"]\ncomponents = [\"b\"]\n", None)]
#[case::unquoted("[toolchain]\ncomponents = [rustfmt]\n", None)]
#[case::empty("[toolchain]\ncomponents = []\n", None)]
fn the_component_list_is_read_or_refused(
    #[case] toolchain: &str,
    #[case] expected: Option<&[&str]>,
) -> Result<(), String> {
    match (pinned_components(toolchain), expected) {
        (Ok(found), Some(wanted)) if found == wanted => Ok(()),
        (Err(_), None) => Ok(()),
        (found, wanted) => Err(format!(
            "for `{toolchain}` the reader said {found:?}, expected {wanted:?}"
        )),
    }
}
