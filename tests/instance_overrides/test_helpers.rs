//! Shared fixtures for instance override behavioural tests.

use rstest::fixture;

use crate::cli_support::{CliOutput, DEFAULT_IMAGE_LABEL, fake_run_command};
use crate::test_constants::DEFAULT_INSTANCE_TYPE;

#[derive(Clone, Debug)]
pub struct CliContext {
    pub default_instance_type: String,
    pub default_image: String,
    pub output: Option<CliOutput>,
}

impl CliContext {
    /// Creates a command for the lazily compiled, feature-enabled test binary.
    /// The executable is reused across scenarios while each call returns a
    /// fresh command.
    ///
    /// The command selects fake request-dump mode and applies this context's
    /// fixture defaults plus dummy Scaleway credentials. Other environment
    /// variables remain inherited from the test process. If Cargo exits with a
    /// failure, `BuildError` retains its status, stdout, and stderr; launch and
    /// artefact-selection failures are also returned as `BuildError`.
    ///
    /// # Example
    ///
    /// Add `run` options to the returned command, then inspect the captured
    /// request dump to verify how instance overrides were represented.
    pub fn base_command(
        &self,
    ) -> Result<assert_cmd::Command, &'static crate::test_binary::BuildError> {
        fake_run_command(&self.default_instance_type, &self.default_image)
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
