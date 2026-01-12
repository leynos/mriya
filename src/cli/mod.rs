//! Command-line interface definitions for the `mriya` binary.
//!
//! This module centralises the clap parser structures so both the main binary
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
