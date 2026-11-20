//! Command-line interface definitions for the `mriya` binary.
//!
//! This module centralizes the clap parser structures so both the main binary
//! and the build script can reuse them when generating the manual page.

use clap::Parser;

/// Top-level CLI for the `mriya` binary.
#[derive(Debug, Parser)]
#[command(
    name = "mriya",
    about = "Teleport your workspace to a Scaleway VM and run commands remotely",
    arg_required_else_help = true
)]
pub(crate) enum Cli {
    /// Provision, sync, and run a command over SSH.
    #[command(name = "run", about = "Provision, sync, and run a command over SSH")]
    Run(RunCommand),
    /// Prepare a cache volume for this project.
    #[command(name = "init", about = "Prepare a cache volume for this project")]
    Init(InitCommand),
}

/// Arguments for the `mriya run` subcommand.
#[derive(Debug, Parser)]
pub(crate) struct RunCommand {
    /// Override the Scaleway instance type (commercial type) for this run.
    ///
    /// The Scaleway backend validates availability in the selected zone during
    /// provisioning, and rejects unknown values with a provider-specific
    /// error.
    #[arg(long, value_name = "TYPE")]
    pub(crate) instance_type: Option<String>,
    /// Override the image label for this run.
    ///
    /// The Scaleway backend resolves the label to a concrete image identifier
    /// for the selected architecture and zone, and rejects unknown labels with
    /// a provider-specific error.
    #[arg(long, value_name = "IMAGE")]
    pub(crate) image: Option<String>,
    /// Provide cloud-init user-data inline for this run (cloud-config YAML or script).
    ///
    /// This payload is passed through to the backend and applied during the
    /// instance's first boot before the remote command is executed.
    #[arg(long, value_name = "USER_DATA", conflicts_with = "cloud_init_file")]
    pub(crate) cloud_init: Option<String>,
    /// Provide cloud-init user-data from a local file for this run.
    #[arg(long, value_name = "PATH", conflicts_with = "cloud_init")]
    pub(crate) cloud_init_file: Option<String>,
    /// Command to execute on the remote host (use -- to separate flags).
    #[arg(required = true, trailing_var_arg = true)]
    pub(crate) command: Vec<String>,
}

/// Arguments for the `mriya init` subcommand.
#[derive(Debug, Parser)]
pub(crate) struct InitCommand {
    /// Overwrite an existing cache volume ID in configuration.
    #[arg(long)]
    pub(crate) force: bool,
}

#[cfg(test)]
mod tests {
    //! Parser tests for the handwritten `run` and `init` commands.

    use clap::error::ErrorKind;

    use super::*;

    fn parse_run(args: &[&str]) -> RunCommand {
        let cli = Cli::try_parse_from(args).expect("run command should parse");
        let Cli::Run(command) = cli else {
            panic!("expected the run subcommand");
        };
        command
    }

    /// Remote flags after `--` are the command, never Mriya options.
    #[test]
    fn run_captures_remote_flags_after_double_dash() {
        let command = parse_run(&["mriya", "run", "--", "--instance-type", "REMOTE"]);

        assert_eq!(command.instance_type, None);
        assert_eq!(command.command, vec!["--instance-type", "REMOTE"]);
    }

    /// An override before `--` applies to Mriya; the same spelling after `--`
    /// stays in the remote command.
    #[test]
    fn run_applies_override_before_double_dash_only() {
        let command = parse_run(&[
            "mriya",
            "run",
            "--instance-type",
            "LOCAL",
            "--",
            "--instance-type",
            "REMOTE",
        ]);

        assert_eq!(command.instance_type.as_deref(), Some("LOCAL"));
        assert_eq!(command.command, vec!["--instance-type", "REMOTE"]);
    }

    /// `mriya init --force` parses, and an unknown flag is rejected.
    #[test]
    fn init_accepts_force_and_rejects_unknown_flags() {
        let cli = Cli::try_parse_from(["mriya", "init", "--force"]).expect("init should parse");
        let Cli::Init(command) = cli else {
            panic!("expected the init subcommand");
        };
        assert!(command.force);

        let error = Cli::try_parse_from(["mriya", "init", "--unknown"])
            .expect_err("unknown flag should be rejected");
        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }

    /// A run without a remote command is rejected.
    #[test]
    fn run_requires_a_remote_command() {
        let error =
            Cli::try_parse_from(["mriya", "run"]).expect_err("missing command should be rejected");

        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }
}
