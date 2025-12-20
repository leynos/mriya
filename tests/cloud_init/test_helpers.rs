//! Shared fixtures for cloud-init behavioural tests.

use std::process::Output;
use std::sync::LazyLock;

use escargot::CargoBuild;
use rstest::fixture;
use tempfile::TempDir;

use crate::test_constants::DEFAULT_INSTANCE_TYPE;

const DEFAULT_IMAGE_LABEL: &str = "Ubuntu 24.04 Noble Numbat";
const DUMMY_SECRET_KEY: &str = "dummy-secret";
const DUMMY_PROJECT_ID: &str = "11111111-2222-3333-4444-555555555555";

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
        let status_code = status.code().unwrap_or(1);
        let stdout = String::from_utf8_lossy(&raw_stdout).into_owned();
        let stderr = String::from_utf8_lossy(&raw_stderr).into_owned();
        Self {
            status_code,
            stdout,
            stderr,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CliContext {
    pub default_instance_type: String,
    pub default_image: String,
    pub output: Option<CliOutput>,
    pub tmp_dir: Option<std::sync::Arc<TempDir>>,
}

#[expect(
    clippy::expect_used,
    reason = "test setup requires panic on build failure"
)]
static MRIYA_BIN: LazyLock<escargot::CargoRun> = LazyLock::new(|| {
    CargoBuild::new()
        .bin("mriya")
        .features("test-backdoors")
        .run()
        .expect("failed to build mriya with test-backdoors feature")
});

pub fn mriya_cmd() -> assert_cmd::Command {
    MRIYA_BIN.command().into()
}

impl CliContext {
    pub fn base_command(&self) -> assert_cmd::Command {
        let mut cmd = mriya_cmd();
        cmd.env("MRIYA_FAKE_RUN_ENABLE", "1");
        cmd.env("MRIYA_FAKE_RUN_MODE", "dump-request");
        cmd.env("SCW_SECRET_KEY", DUMMY_SECRET_KEY);
        cmd.env("SCW_DEFAULT_PROJECT_ID", DUMMY_PROJECT_ID);
        cmd.env("SCW_DEFAULT_INSTANCE_TYPE", &self.default_instance_type);
        cmd.env("SCW_DEFAULT_IMAGE", &self.default_image);
        cmd
    }
}

#[fixture]
pub fn cli_context() -> CliContext {
    CliContext {
        default_instance_type: String::from(DEFAULT_INSTANCE_TYPE),
        default_image: String::from(DEFAULT_IMAGE_LABEL),
        output: None,
        tmp_dir: None,
    }
}
