//! Shared fixtures for cloud-init behavioural tests.

use std::ffi::OsStr;

use rstest::fixture;

use crate::{
    isolated_cli::{
        CliOutput,
        DEFAULT_IMAGE_LABEL,
        IsolatedRun,
        RunEnvironment,
        isolated_run_command,
    },
    test_constants::DEFAULT_INSTANCE_TYPE,
};

#[derive(Clone, Debug)]
pub struct CliContext {
    pub default_instance_type: String,
    pub default_image: String,
    pub output: Option<CliOutput>,
}

impl CliContext {
    /// Starts one isolated `mriya run` invocation.
    ///
    /// The executable is the ordinary binary, reused across scenarios, while
    /// each call returns a fresh command with this context's fixture defaults.
    /// Every inherited environment variable is cleared, so only the isolated
    /// values and the scenario's own overrides reach the child. `home`
    /// replaces the isolated temporary home when a scenario exercises an
    /// unusual one.
    ///
    /// # Errors
    ///
    /// Returns an error when the binary cannot be built or the isolation
    /// directories cannot be created.
    ///
    /// # Example
    ///
    /// Pass the scenario's `--cloud-init` options to
    /// [`IsolatedRun::execute`] to observe how the binary handled them.
    pub fn base_command(&self, home: Option<&OsStr>) -> anyhow::Result<IsolatedRun> {
        isolated_run_command(&RunEnvironment {
            default_instance_type: &self.default_instance_type,
            default_image: &self.default_image,
            home,
            variables: &[],
        })
    }
}

#[fixture]
pub fn cli_context() -> CliContext {
    CliContext {
        default_instance_type: String::from(DEFAULT_INSTANCE_TYPE),
        default_image: String::from(DEFAULT_IMAGE_LABEL),
        output: None,
    }
}
