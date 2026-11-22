//! An isolated `mriya run` invocation, shared by the CLI suites.
//!
//! The binary under test is the one `cargo build --bin mriya` produces with no
//! features, so these suites observe the code a user receives. Isolation is
//! two-sided: the child inherits none of the test process's environment, and
//! every run is stopped before `backend.create` by an argument the CLI
//! rejects. A suite therefore cannot reach Scaleway, however it is configured.

use std::{ffi::OsStr, io, process::Output};

use anyhow::Context as _;
use tempfile::{NamedTempFile, TempDir};

/// The image label the CLI selects when a scenario does not set one.
///
/// It must match `ScalewayConfig::default_image`; a scenario that changes the
/// config default without changing this constant asserts against an image the
/// binary never resolves.
pub const DEFAULT_IMAGE_LABEL: &str = "Ubuntu 24.04 Noble Numbat";

const DUMMY_SECRET_KEY: &str = "dummy-secret";
const DUMMY_PROJECT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The remote-command argument that stops every isolated run.
///
/// It contains a real tab, which `mriya`'s argument validation rejects before
/// the backend can create anything. Appending it is what makes the suites fail
/// closed: forgetting to add an override or a cloud-init file produces a
/// diagnostic, never a billable instance.
const PROVISIONING_STOP_ARGUMENT: &str = "fail\tclosed";

/// Captured result of one CLI invocation.
#[derive(Clone, Debug)]
pub struct CliOutput {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    pub fn from_process_output(output: Output) -> Self {
        let Output {
            status,
            stdout: raw_stdout,
            stderr: raw_stderr,
        } = output;
        Self {
            status_code: status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&raw_stdout).into_owned(),
            stderr: String::from_utf8_lossy(&raw_stderr).into_owned(),
        }
    }
}

/// Describes how one isolated `mriya run` is built.
///
/// The field names state what a suite is varying; everything not named here is
/// fixed by the isolation itself.
pub struct RunEnvironment<'a> {
    /// The instance type the suite treats as the configured default.
    pub default_instance_type: &'a str,
    /// The image label the suite treats as the configured default.
    pub default_image: &'a str,
    /// A `HOME` override for the child.
    ///
    /// Only a scenario that exercises tilde expansion or an undecodable value
    /// supplies one; otherwise the child sees the isolated temporary home.
    pub home: Option<&'a OsStr>,
    /// Extra variables applied after the isolated values.
    ///
    /// A regression case arms a retired `MRIYA_FAKE_*` control here.
    pub variables: &'a [(&'a str, &'a str)],
}

/// One `mriya run` invocation that cannot reach the machine or the cloud.
///
/// The command starts from an empty environment holding only the isolated
/// values, and completes with [`PROVISIONING_STOP_ARGUMENT`] appended. The
/// temporary directories live here, so they outlive the child process that
/// reads them.
pub struct IsolatedRun {
    command: assert_cmd::Command,
    _isolation: Isolation,
}

impl IsolatedRun {
    /// Runs the CLI with `options` before the remote command.
    ///
    /// The fail-closed argument is appended here rather than by the caller, so
    /// there is no way for a suite to run the binary with a reachable backend.
    ///
    /// # Errors
    ///
    /// Returns the failure to spawn the binary or to collect its output.
    pub fn execute(mut self, options: &[&str]) -> io::Result<Output> {
        self.command.args(options);
        self.command
            .args(["--", "echo", PROVISIONING_STOP_ARGUMENT]);
        self.command.output()
    }
}

/// Creates a `run` command for the ordinary `mriya` binary, isolated from the
/// machine running the tests.
///
/// The child inherits no environment variables. It receives temporary `HOME`,
/// `XDG_CONFIG_HOME` and `XDG_CONFIG_DIRS` directories, an empty configuration
/// file for `MRIYA_CONFIG_PATH`, the dummy Scaleway credentials, the given
/// fixture defaults, and a fresh temporary working directory. Without that
/// isolation a developer's own `mriya.toml` or `SCW_*` variables would change
/// what the binary reads, and an assertion would describe that machine rather
/// than the code.
///
/// # Errors
///
/// Returns an error when the binary cannot be built or an isolation directory
/// cannot be created.
///
/// # Example
///
/// Build an environment, then complete it with [`IsolatedRun::execute`] to
/// observe how the binary handled the options. The invocation stops before
/// provisioning, so only preparation failures are observable.
pub fn isolated_run_command(environment: &RunEnvironment<'_>) -> anyhow::Result<IsolatedRun> {
    let mut command = crate::test_binary::mriya_cmd()
        .map_err(|err| anyhow::anyhow!("failed to build the mriya binary: {err}"))?;
    let isolation = Isolation::create().context("failed to create isolation directories")?;

    command
        .env_clear()
        .env("SCW_SECRET_KEY", DUMMY_SECRET_KEY)
        .env("SCW_DEFAULT_PROJECT_ID", DUMMY_PROJECT_ID)
        .env(
            "SCW_DEFAULT_INSTANCE_TYPE",
            environment.default_instance_type,
        )
        .env("SCW_DEFAULT_IMAGE", environment.default_image)
        .arg("run");
    isolation.apply(&mut command);
    if let Some(home) = environment.home {
        command.env("HOME", home);
    }
    for (key, value) in environment.variables {
        command.env(key, value);
    }

    Ok(IsolatedRun {
        command,
        _isolation: isolation,
    })
}

/// The temporary directories and empty configuration file that make one run
/// independent of the machine it runs on.
struct Isolation {
    working_directory: TempDir,
    home: TempDir,
    config_home: TempDir,
    config_dirs: TempDir,
    config_file: NamedTempFile,
}

impl Isolation {
    /// Creates every temporary location one run needs.
    ///
    /// The configuration file carries a `.toml` suffix so the loader reads it
    /// with the format it was written in; its contents are empty, which is a
    /// valid configuration with no keys.
    fn create() -> io::Result<Self> {
        Ok(Self {
            working_directory: TempDir::new()?,
            home: TempDir::new()?,
            config_home: TempDir::new()?,
            config_dirs: TempDir::new()?,
            config_file: tempfile::Builder::new().suffix(".toml").tempfile()?,
        })
    }

    /// Points `command` at these locations and nowhere else.
    fn apply(&self, command: &mut assert_cmd::Command) {
        command
            .current_dir(self.working_directory.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.config_home.path())
            .env("XDG_CONFIG_DIRS", self.config_dirs.path())
            .env("MRIYA_CONFIG_PATH", self.config_file.path());
    }
}
