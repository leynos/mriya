//! Integration tests for layered configuration loading.
//!
//! These tests drive the generated `ortho_config` loaders with explicit
//! environment and file sources (`MapEnv` and `MRIYA_CONFIG_PATH`) instead of
//! mutating the process environment, so they stay isolated while running in
//! parallel. They pin the file shape, the precedence order, the v0.10.0
//! explicit-command-line fix, and boolean flag spelling.

mod config_support;

use anyhow::{bail, ensure};
use config_support::{Workspace, shared_env};
use mriya::{ScalewayConfig, SyncConfig};
use ortho_config::OrthoConfig;
use serde::Deserialize;

fn collected_env<'a>(
    workspace: &'a Workspace,
    extra: &'a [(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    let mut vars = vec![
        ("HOME", workspace.home.as_str()),
        ("MRIYA_CONFIG_PATH", workspace.config.as_str()),
    ];
    vars.extend_from_slice(extra);
    vars
}

fn load_sync(
    workspace: &Workspace,
    extra_env: &[(&str, &str)],
    extra_args: &[&str],
) -> anyhow::Result<SyncConfig> {
    let vars = collected_env(workspace, extra_env);
    let (discovery, merge) = shared_env(&vars);
    let mut args = vec!["mriya"];
    args.extend_from_slice(extra_args);
    SyncConfig::load_from_iter_with_sources(args, discovery, merge)
        .map_err(|err| anyhow::anyhow!("sync configuration failed: {err}"))
}

fn load_scaleway(
    workspace: &Workspace,
    extra_env: &[(&str, &str)],
    extra_args: &[&str],
) -> anyhow::Result<ScalewayConfig> {
    let vars = collected_env(workspace, extra_env);
    let (discovery, merge) = shared_env(&vars);
    let mut args = vec!["mriya"];
    args.extend_from_slice(extra_args);
    ScalewayConfig::load_from_iter_with_sources(args, discovery, merge)
        .map_err(|err| anyhow::anyhow!("scaleway configuration failed: {err}"))
}

/// A minimal locally derived configuration used to reach the whole-command-line
/// condition that exposed the v0.10.0 explicit-value fix.
#[derive(Debug, Deserialize, OrthoConfig)]
#[ortho_config(
    prefix = "MRIYA_PROBE",
    discovery(
        app_name = "mriya-probe",
        env_var = "MRIYA_PROBE_CONFIG_PATH",
        config_file_name = "mriya-probe.toml",
        dotfile_name = ".mriya-probe.toml",
        project_file_name = "mriya-probe.toml"
    )
)]
struct ProbeConfig {
    #[ortho_config(default = true)]
    enabled: bool,
    #[ortho_config(default = "default".to_owned())]
    label: String,
}

fn load_probe(
    workspace: &Workspace,
    extra_env: &[(&str, &str)],
    args: &[&str],
) -> anyhow::Result<ProbeConfig> {
    // Selecting an absent path suppresses project-file discovery, so a stray
    // `mriya-probe.toml` in the working directory cannot affect the result.
    let missing = workspace.root.join("missing-probe.toml");
    let mut vars = vec![
        ("HOME", workspace.home.as_str()),
        ("XDG_CONFIG_HOME", workspace.home.as_str()),
        ("XDG_CONFIG_DIRS", workspace.home.as_str()),
        ("MRIYA_PROBE_CONFIG_PATH", missing.as_str()),
    ];
    vars.extend_from_slice(extra_env);
    let (discovery, merge) = shared_env(&vars);
    ProbeConfig::load_from_iter_with_sources(args, discovery, merge)
        .map_err(|err| anyhow::anyhow!("probe configuration failed: {err}"))
}

#[test]
fn sync_config_reads_root_level_file_keys() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write("mriya.toml", "volume_mount_path = \"/from-file\"\n")?;

    let config = load_sync(&workspace, &[], &[])?;

    ensure!(
        config.volume_mount_path == "/from-file",
        "root-level key should load, got {}",
        config.volume_mount_path
    );
    Ok(())
}

#[test]
fn sync_config_ignores_sectioned_file_keys() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write(
        "mriya.toml",
        "[sync]\nvolume_mount_path = \"/from-section\"\n",
    )?;

    let config = load_sync(&workspace, &[], &[])?;

    ensure!(
        config.volume_mount_path == mriya::sync::DEFAULT_VOLUME_MOUNT_PATH,
        "sectioned key must be ignored, got {}",
        config.volume_mount_path
    );
    Ok(())
}

#[test]
fn scaleway_config_reads_root_level_file_keys() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write(
        "mriya.toml",
        concat!(
            "secret_key = \"secret\"\n",
            "default_project_id = \"project\"\n",
            "default_zone = \"nl-ams-1\"\n"
        ),
    )?;

    let config = load_scaleway(&workspace, &[], &[])?;

    ensure!(config.secret_key == "secret", "secret key should load");
    ensure!(
        config.default_zone == "nl-ams-1",
        "zone should load, got {}",
        config.default_zone
    );
    Ok(())
}

#[test]
fn scaleway_config_ignores_sectioned_file_keys() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write(
        "mriya.toml",
        "[scaleway]\nsecret_key = \"secret\"\ndefault_project_id = \"project\"\n",
    )?;

    let Err(err) = load_scaleway(&workspace, &[], &[]) else {
        bail!("a sectioned file must not satisfy required credentials");
    };
    let message = err.to_string();

    ensure!(
        message.contains("secret_key"),
        "error should name the missing root-level key: {message}"
    );
    Ok(())
}

#[test]
fn file_value_overrides_default() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write("mriya.toml", "volume_mount_path = \"/from-file\"\n")?;

    let config = load_sync(&workspace, &[], &[])?;

    ensure!(config.volume_mount_path == "/from-file");
    Ok(())
}

#[test]
fn environment_value_overrides_file() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write("mriya.toml", "volume_mount_path = \"/from-file\"\n")?;

    let config = load_sync(
        &workspace,
        &[("MRIYA_SYNC_VOLUME_MOUNT_PATH", "/from-env")],
        &[],
    )?;

    ensure!(
        config.volume_mount_path == "/from-env",
        "environment should win over the file, got {}",
        config.volume_mount_path
    );
    Ok(())
}

#[test]
fn command_line_overrides_environment() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write("mriya.toml", "volume_mount_path = \"/from-file\"\n")?;

    let config = load_sync(
        &workspace,
        &[("MRIYA_SYNC_VOLUME_MOUNT_PATH", "/from-env")],
        &["--volume-mount-path", "/from-cli"],
    )?;

    ensure!(
        config.volume_mount_path == "/from-cli",
        "command line should win over the environment, got {}",
        config.volume_mount_path
    );
    Ok(())
}

#[test]
fn default_value_applies_without_other_sources() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;

    let config = load_sync(&workspace, &[], &[])?;

    ensure!(config.volume_mount_path == mriya::sync::DEFAULT_VOLUME_MOUNT_PATH);
    Ok(())
}

#[test]
fn explicit_command_line_value_equal_to_default_wins() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;

    // The command line restates every default, so the sanitized CLI object
    // equals the defaults object. The v0.9.0 layering guard dropped the CLI
    // layer in this case; v0.10.0 asks clap per argument and keeps it.
    let config = load_probe(
        &workspace,
        &[("MRIYA_PROBE_ENABLED", "false")],
        &["probe", "--enabled", "--label", "default"],
    )?;

    ensure!(config.label == "default", "label should use the CLI value");
    ensure!(
        config.enabled,
        "explicit --enabled equal to the default must beat the environment"
    );
    Ok(())
}

#[test]
fn environment_value_applies_when_command_line_is_absent() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;

    let config = load_probe(&workspace, &[("MRIYA_PROBE_ENABLED", "false")], &["probe"])?;

    ensure!(
        !config.enabled,
        "the environment must still win when no flag is supplied"
    );
    Ok(())
}

#[test]
fn boolean_flag_accepts_explicit_false() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;

    let config = load_probe(
        &workspace,
        &[("MRIYA_PROBE_ENABLED", "true")],
        &["probe", "--enabled=false"],
    )?;

    ensure!(
        !config.enabled,
        "--enabled=false must clear a true from the environment"
    );
    Ok(())
}

#[test]
fn boolean_flag_rejects_space_separated_value() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;

    let Err(err) = load_probe(&workspace, &[], &["probe", "--enabled", "false"]) else {
        bail!("the space-separated boolean spelling must be rejected");
    };
    let message = err.to_string();

    ensure!(
        message.contains("unexpected argument 'false'"),
        "error should report the unexpected positional value: {message}"
    );
    Ok(())
}
