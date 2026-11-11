//! The behavioural half of the build-tools contract: the preflight and installer scripts and the
//! Make routes that run them, executed against a sandbox `PATH` holding exactly the tools a case
//! chooses. Dry-run text proves what Make would print; these prove the preflight stops a route that
//! lacks a tool, tells the developer how to fix it, and lets a complete toolset through to Cargo.

use std::process::Output;

use super::{
    make::Target,
    pins::{TOOLCHAIN, pinned_channel, pinned_linker},
    sandbox::{CARGO_LOG, LINKER_TOOL, Sandbox},
};

/// Host utilities the scripts and `make` itself use; the sandbox links these and nothing else.
const UTILITIES: &[&str] = &[
    "bash", "env", "uname", "dirname", "grep", "awk", "tr", "mktemp", "rm", "sed", "cat", "sh",
];

/// A `mold --version` the sandbox can report.
#[derive(Clone, Copy)]
pub enum Linker {
    /// The release `tools/mold/VERSION` pins.
    Pinned,
    /// A version deliberately off the pin, to prove the preflight compares them.
    Drifted(&'static str),
}

/// What the sandbox offers the preflight.
#[derive(Clone, Copy)]
pub struct Toolset {
    /// The version `mold --version` reports, or `None` for no `mold` on `PATH`.
    pub linker: Option<Linker>,
    /// Whether `cc` can link with `-fuse-ld=mold`; `None` leaves out `cc` altogether.
    pub cc_links: Option<bool>,
    /// Whether `rustup` lists the pinned toolchain.
    pub has_toolchain: bool,
    /// Whether the host can answer `uname -s`; `false` makes platform detection fail.
    pub platform_answers: bool,
}

impl Toolset {
    /// Everything present, at the pins.
    pub const COMPLETE: Self = Self {
        linker: Some(Linker::Pinned),
        cc_links: Some(true),
        has_toolchain: true,
        platform_answers: true,
    };
}

/// Builds a sandbox holding the utilities and the fakes `toolset` describes, plus a `probe-cargo`
/// that logs its arguments.
///
/// # Errors
///
/// Returns the reason when a utility is missing from the host or a fake cannot be written.
pub fn sandbox_with(toolset: Toolset) -> Result<Sandbox, String> {
    let sandbox = Sandbox::new()?;
    sandbox.link_tools(UTILITIES)?;
    if let Some(linker) = toolset.linker {
        let version = match linker {
            Linker::Pinned => pinned_linker(),
            Linker::Drifted(version) => version,
        };
        sandbox.write_tool(
            LINKER_TOOL,
            format!("echo '{LINKER_TOOL} {version} (compatible with GNU ld)'"),
        )?;
    }
    if let Some(links) = toolset.cc_links {
        sandbox.write_tool("cc", fake_cc(links))?;
    }
    let listed = if toolset.has_toolchain {
        let channel = pinned_channel(TOOLCHAIN)?;
        format!("echo '{channel}-x86_64-unknown-linux-gnu (active, default)'")
    } else {
        "echo 'stable-x86_64-unknown-linux-gnu (default)'".to_owned()
    };
    sandbox.write_tool("rustup", listed)?;
    sandbox.write_tool("probe-cargo", "echo \"$*\" >> \"$CARGO_LOG\"")?;
    if !toolset.platform_answers {
        sandbox.pretend_uname_fails()?;
    }
    Ok(sandbox)
}

/// A `cc` whose link succeeds only when the preflight passes `-fuse-ld=mold`.
///
/// The preflight's whole purpose is to prove the driver accepts the flag the
/// build standard gives it. A fake that ignored its arguments would pass with
/// the flag removed, so it would prove nothing; this one fails in that case,
/// making the probe's use of the flag observable.
///
/// `links` is whether the driver can link at the pin: true accepts the flag,
/// false rejects it, and either way a probe without the flag fails.
fn fake_cc(links: bool) -> String {
    let with_flag = i32::from(!links);
    // A probe that omits the flag always fails, whatever the driver can do
    // with it, so the case for a driver that rejects the flag keeps failing
    // for the reason it is named after rather than for the missing flag.
    let without_flag = 1;
    format!(
        "cat >/dev/null\n\
         for arg in \"$@\"; do\n    \
         if [ \"$arg\" = '-fuse-ld=mold' ]; then\n        \
         echo 'cc: fake link with -fuse-ld=mold' >&2\n        \
         exit {with_flag}\n    \
         fi\ndone\n\
         echo 'cc: fake link without -fuse-ld=mold' >&2\nexit {without_flag}\n"
    )
}

/// Returns a process's stderr as text, for a message to quote.
fn stderr_of(output: &Output) -> String { String::from_utf8_lossy(&output.stderr).into_owned() }

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

/// Runs one preflight script in the sandbox, from a path the caller supplies.
///
/// # Errors
///
/// Returns the reason when the script cannot be started.
pub fn run_preflight_script(
    sandbox: &Sandbox,
    script: impl AsRef<std::path::Path>,
) -> Result<Output, String> {
    sandbox
        .command("bash")?
        .arg(script.as_ref())
        .output()
        .map_err(|error| format!("running the preflight: {error}"))
}

/// Runs a Make route for real in the sandbox, with the fake Cargo substituted.
///
/// # Errors
///
/// Returns the reason when `make` cannot be started.
pub fn run_route(sandbox: &Sandbox, route: Target<'_>) -> Result<Output, String> {
    sandbox
        .command("make")?
        .args(["-B", "CARGO=probe-cargo", route.name()])
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
