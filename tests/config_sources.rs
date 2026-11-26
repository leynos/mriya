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
use rstest::rstest;
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

/// Loads a configuration type through the generated loader with injected
/// sources, naming the type in any failure.
fn load<T>(
    workspace: &Workspace,
    label: &str,
    extra_env: &[(&str, &str)],
    extra_args: &[&str],
) -> anyhow::Result<T>
where
    T: OrthoConfig,
{
    let vars = collected_env(workspace, extra_env);
    let (discovery, merge) = shared_env(&vars);
    let mut args = vec!["mriya"];
    args.extend_from_slice(extra_args);
    T::load_from_iter_with_sources(args, discovery, merge)
        .map_err(|err| anyhow::anyhow!("{label} configuration failed: {err}"))
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

/// The volume mount path seen at each tier, with each source winning over the
/// one below it. The sectioned-file row proves the file layer ignores a
/// `[sync]` table even when it holds the same key.
#[rstest]
#[case::default_value(None, &[], &[], mriya::sync::DEFAULT_VOLUME_MOUNT_PATH)]
#[case::root_level_file(
    Some("volume_mount_path = \"/from-file\"\n"),
    &[],
    &[],
    "/from-file",
)]
#[case::sectioned_file_is_ignored(
    Some("[sync]\nvolume_mount_path = \"/from-section\"\n"),
    &[],
    &[],
    mriya::sync::DEFAULT_VOLUME_MOUNT_PATH,
)]
#[case::environment_overrides_file(
    Some("volume_mount_path = \"/from-file\"\n"),
    &[("MRIYA_SYNC_VOLUME_MOUNT_PATH", "/from-env")],
    &[],
    "/from-env",
)]
#[case::command_line_overrides_environment(
    Some("volume_mount_path = \"/from-file\"\n"),
    &[("MRIYA_SYNC_VOLUME_MOUNT_PATH", "/from-env")],
    &["--volume-mount-path", "/from-cli"],
    "/from-cli",
)]
fn sync_volume_mount_path_follows_the_precedence_order(
    #[case] file: Option<&str>,
    #[case] extra_env: &[(&str, &str)],
    #[case] extra_args: &[&str],
    #[case] expected: &str,
) -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    if let Some(contents) = file {
        workspace.write("mriya.toml", contents)?;
    }

    let config = load::<SyncConfig>(&workspace, "sync", extra_env, extra_args)?;

    ensure!(
        config.volume_mount_path == expected,
        "expected {expected}, got {}",
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

    let config = load::<ScalewayConfig>(&workspace, "scaleway", &[], &[])?;

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

    let Err(err) = load::<ScalewayConfig>(&workspace, "scaleway", &[], &[]) else {
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
