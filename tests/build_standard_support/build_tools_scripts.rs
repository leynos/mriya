//! The behavioural half of the build-tools contract: the preflight and installer scripts and the Make
//! routes that run them, executed against a sandbox `PATH` holding exactly the tools a case chooses.
//! Dry-run text proves what Make would print; these prove the preflight stops a route that lacks a
//! tool, tells the developer how to fix it, and lets a complete toolset through to Cargo.

use std::process::Output;

use super::sandbox::{CARGO_LOG, Sandbox};

/// The mold linker release the repository pins, and the toolchain the preflight looks for.
const PINNED_LINKER: &str = "2.41.0";
const PINNED_TOOLCHAIN: &str = "nightly-2026-11-06";
/// Host utilities the scripts and `make` itself use; the sandbox links these and nothing else.
const UTILITIES: &[&str] = &[
    "bash", "env", "uname", "dirname", "grep", "awk", "tr", "mktemp", "rm", "sed", "cat", "sh",
];

/// What the sandbox offers the preflight.
#[derive(Clone, Copy)]
pub struct Toolset {
    /// The version `mold --version` reports, or `None` for no `mold` on `PATH`.
    pub linker: Option<&'static str>,
    /// Whether `cc` can link with `-fuse-ld=mold`; `None` leaves out `cc` altogether.
    pub cc_links: Option<bool>,
    /// Whether `rustup` lists the pinned toolchain.
    pub has_toolchain: bool,
}

impl Toolset {
    /// Everything present, at the pins.
    pub const COMPLETE: Self = Self {
        linker: Some(PINNED_LINKER),
        cc_links: Some(true),
        has_toolchain: true,
    };
}

/// Builds a sandbox holding the utilities and the fakes `toolset` describes, plus a `probe-cargo` that
/// logs its arguments.
///
/// # Errors
///
/// Returns the reason when a utility is missing from the host or a fake cannot be written.
pub fn sandbox_with(toolset: Toolset) -> Result<Sandbox, String> {
    let sandbox = Sandbox::new()?;
    sandbox.link_tools(UTILITIES)?;
    if let Some(version) = toolset.linker {
        sandbox.write_tool(
            "mold",
            &format!("echo 'mold {version} (compatible with GNU ld)'"),
        )?;
    }
    if let Some(links) = toolset.cc_links {
        let status = i32::from(!links);
        let body = format!("cat >/dev/null; echo 'cc: fake link run' >&2; exit {status}");
        sandbox.write_tool("cc", &body)?;
    }
    let listed = if toolset.has_toolchain {
        format!("echo '{PINNED_TOOLCHAIN}-x86_64-unknown-linux-gnu (active, default)'")
    } else {
        "echo 'stable-x86_64-unknown-linux-gnu (default)'".to_owned()
    };
    sandbox.write_tool("rustup", &listed)?;
    sandbox.write_tool("probe-cargo", "echo \"$*\" >> \"$CARGO_LOG\"")?;
    Ok(sandbox)
}

/// Returns a process's stderr as text, for a message to quote.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Runs the preflight script directly in the sandbox.
///
/// # Errors
///
/// Returns the reason when the script cannot be started.
pub fn run_preflight(sandbox: &Sandbox) -> Result<Output, String> {
    sandbox
        .command("bash")?
        .arg("scripts/check-build-tools.sh")
        .output()
        .map_err(|error| format!("running the preflight: {error}"))
}

/// Runs a Make route for real in the sandbox, with the fake Cargo substituted.
///
/// # Errors
///
/// Returns the reason when `make` cannot be started.
pub fn run_route(sandbox: &Sandbox, route: &str) -> Result<Output, String> {
    sandbox
        .command("make")?
        .args(["-B", "CARGO=probe-cargo", route])
        .output()
        .map_err(|error| format!("running make {route}: {error}"))
}

/// Checks that a preflight run failed and told the developer how to put it right.
///
/// # Errors
///
/// Returns the reason when it succeeded, or stayed silent about the fix.
pub fn expect_actionable_failure(output: &Output, fragments: &[&str]) -> Result<(), String> {
    let stderr = stderr_of(output);
    if output.status.success() {
        return Err(format!("the preflight passed but should fail:\n{stderr}"));
    }
    for fragment in fragments {
        if !stderr.contains(fragment) {
            return Err(format!("the failure never says `{fragment}`:\n{stderr}"));
        }
    }
    Ok(())
}

/// Returns the lines the fake Cargo logged, in order; empty when it never ran.
///
/// # Errors
///
/// Returns the reason when the log exists but cannot be read.
pub fn cargo_calls(sandbox: &Sandbox) -> Result<Vec<String>, String> {
    let log = sandbox.read(CARGO_LOG)?.unwrap_or_default();
    Ok(log.lines().map(str::to_owned).collect())
}
