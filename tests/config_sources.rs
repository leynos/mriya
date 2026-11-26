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

/// A configuration file holding a root-level sync value.
const SYNC_ROOT_FILE: &str = "volume_mount_path = \"/from-file\"\n";
/// The same key wrapped in a `[sync]` table, which the loader must ignore.
const SYNC_SECTION_FILE: &str = "[sync]\nvolume_mount_path = \"/from-section\"\n";
/// The credentials every Scaleway load needs, with no zone set.
const SCW_CREDENTIALS: &str = "secret_key = \"secret\"\ndefault_project_id = \"project\"\n";
/// The credentials plus a root-level zone.
const SCW_ZONE_FILE: &str = concat!(
    "secret_key = \"secret\"\n",
    "default_project_id = \"project\"\n",
    "default_zone = \"nl-ams-1\"\n"
);

/// Which configuration struct a layered scenario loads, and the one field it
/// compares.
#[derive(Clone, Copy)]
enum Subject {
    Sync,
    Scaleway,
}

impl Subject {
    /// The name used in a load failure, so a broken case names its subject.
    fn label(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Scaleway => "scaleway",
        }
    }
}

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
/// sources, naming the subject in any failure.
fn load<T>(
    workspace: &Workspace,
    subject: Subject,
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
        .map_err(|err| anyhow::anyhow!("{} configuration failed: {err}", subject.label()))
}

/// Loads `subject` and returns the value the precedence rows compare.
fn layered_value(
    subject: Subject,
    workspace: &Workspace,
    extra_env: &[(&str, &str)],
    extra_args: &[&str],
) -> anyhow::Result<String> {
    Ok(match subject {
        Subject::Sync => {
            load::<SyncConfig>(workspace, subject, extra_env, extra_args)?.volume_mount_path
        }
        Subject::Scaleway => {
            load::<ScalewayConfig>(workspace, subject, extra_env, extra_args)?.default_zone
        }
    })
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

/// One precedence row: what to load, what to write, and what must win.
#[derive(Clone, Copy)]
struct Scenario {
    /// The configuration struct and the field the row compares.
    subject: Subject,
    /// The configuration file contents, or `None` for no file.
    file: Option<&'static str>,
    /// Extra environment variables for the row.
    env: &'static [(&'static str, &'static str)],
    /// Extra command-line arguments for the row.
    args: &'static [&'static str],
    /// The value the highest supplied source must produce.
    expected: &'static str,
}

/// Each source wins over the one below it: default, file, environment, then
/// command line. Sync rows compare the volume mount path; Scaleway rows
/// compare the zone, a non-secret field with a default on a struct whose
/// credentials are otherwise required. The sectioned rows prove the file
/// layer ignores `[sync]` and `[scaleway]` tables.
#[rstest]
#[case::sync_default(Scenario {
    subject: Subject::Sync,
    file: None,
    env: &[],
    args: &[],
    expected: mriya::sync::DEFAULT_VOLUME_MOUNT_PATH,
})]
#[case::sync_root_file(Scenario {
    subject: Subject::Sync,
    file: Some(SYNC_ROOT_FILE),
    env: &[],
    args: &[],
    expected: "/from-file",
})]
#[case::sync_sectioned_file(Scenario {
    subject: Subject::Sync,
    file: Some(SYNC_SECTION_FILE),
    env: &[],
    args: &[],
    expected: mriya::sync::DEFAULT_VOLUME_MOUNT_PATH,
})]
#[case::sync_environment_overrides_file(Scenario {
    subject: Subject::Sync,
    file: Some(SYNC_ROOT_FILE),
    env: &[("MRIYA_SYNC_VOLUME_MOUNT_PATH", "/from-env")],
    args: &[],
    expected: "/from-env",
})]
#[case::sync_command_line_overrides_environment(Scenario {
    subject: Subject::Sync,
    file: Some(SYNC_ROOT_FILE),
    env: &[("MRIYA_SYNC_VOLUME_MOUNT_PATH", "/from-env")],
    args: &["--volume-mount-path", "/from-cli"],
    expected: "/from-cli",
})]
#[case::scaleway_default_zone(Scenario {
    subject: Subject::Scaleway,
    file: Some(SCW_CREDENTIALS),
    env: &[],
    args: &[],
    expected: "fr-par-1",
})]
#[case::scaleway_file_zone(Scenario {
    subject: Subject::Scaleway,
    file: Some(SCW_ZONE_FILE),
    env: &[],
    args: &[],
    expected: "nl-ams-1",
})]
#[case::scaleway_environment_overrides_file(Scenario {
    subject: Subject::Scaleway,
    file: Some(SCW_ZONE_FILE),
    env: &[("SCW_DEFAULT_ZONE", "fr-par-2")],
    args: &[],
    expected: "fr-par-2",
})]
#[case::scaleway_command_line_overrides_environment(Scenario {
    subject: Subject::Scaleway,
    file: Some(SCW_ZONE_FILE),
    env: &[("SCW_DEFAULT_ZONE", "fr-par-2")],
    args: &["--default-zone", "pl-waw-1"],
    expected: "pl-waw-1",
})]
fn layered_configuration_precedence(#[case] scenario: Scenario) -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    if let Some(contents) = scenario.file {
        workspace.write("mriya.toml", contents)?;
    }

    let value = layered_value(scenario.subject, &workspace, scenario.env, scenario.args)?;

    ensure!(
        value == scenario.expected,
        "expected {}, got {value}",
        scenario.expected
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

    let config = load::<ScalewayConfig>(&workspace, Subject::Scaleway, &[], &[])?;

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

    let Err(err) = load::<ScalewayConfig>(&workspace, Subject::Scaleway, &[], &[]) else {
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
