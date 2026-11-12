//! Integration tests for configuration discovery and persisted volume IDs.
//!
//! These tests drive real discovery and the `ConfigStore` against a temporary
//! workspace. They pin how absent, malformed, and unreadable configuration
//! files are reported, and that the store writes a volume ID that
//! `ScalewayConfig` reads back.

mod config_support;

use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use cap_std::ambient_authority;
use config_support::{Workspace, path_text, shared_env};
use mriya::{ConfigStore, ConfigWriter, ScalewayConfig, SyncConfig};
use ortho_config::{ConfigDiscovery, MapEnv, OrthoConfig};

fn load_sync_with_env(vars: &[(&str, &str)]) -> anyhow::Result<SyncConfig> {
    let (discovery, merge) = shared_env(vars);
    SyncConfig::load_from_iter_with_sources(["mriya"], discovery, merge)
        .map_err(|err| anyhow::anyhow!("sync configuration failed: {err}"))
}

fn load_scaleway(workspace: &Workspace) -> anyhow::Result<ScalewayConfig> {
    let vars = [
        ("HOME", workspace.home.as_str()),
        ("MRIYA_CONFIG_PATH", workspace.config.as_str()),
    ];
    let (discovery, merge) = shared_env(&vars);
    ScalewayConfig::load_from_iter_with_sources(["mriya"], discovery, merge)
        .map_err(|err| anyhow::anyhow!("scaleway configuration failed: {err}"))
}

fn store_for(workspace: &Workspace) -> ConfigStore {
    let env = Arc::new(MapEnv::new().with_var("MRIYA_CONFIG_PATH", workspace.config.as_str()));
    let discovery = ConfigDiscovery::builder("mriya")
        .env_var("MRIYA_CONFIG_PATH")
        .config_file_name("mriya.toml")
        .dotfile_name(".mriya.toml")
        .project_file_name("mriya.toml")
        .env_source(env)
        .clear_project_roots()
        .add_project_root(&workspace.root)
        .build();
    ConfigStore::with_discovery(discovery)
}

#[test]
fn missing_optional_config_file_is_not_an_error() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    // No file is written, and no MRIYA_CONFIG_PATH is supplied.
    let vars = [
        ("HOME", workspace.home.as_str()),
        ("XDG_CONFIG_HOME", workspace.home.as_str()),
        ("XDG_CONFIG_DIRS", workspace.home.as_str()),
    ];

    let config = load_sync_with_env(&vars)
        .context("an absent optional file should fall back to defaults")?;

    ensure!(config.volume_mount_path == mriya::sync::DEFAULT_VOLUME_MOUNT_PATH);
    Ok(())
}

#[test]
fn missing_explicit_config_file_falls_back_to_defaults() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    let missing = path_text(&workspace.root.join("missing.toml"));
    let vars = [
        ("HOME", workspace.home.as_str()),
        ("MRIYA_CONFIG_PATH", missing.as_str()),
    ];

    let config = load_sync_with_env(&vars)
        .context("a missing environment-selected file should fall back")?;

    ensure!(config.volume_mount_path == mriya::sync::DEFAULT_VOLUME_MOUNT_PATH);
    Ok(())
}

#[test]
fn malformed_config_file_is_reported() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write("mriya.toml", "volume_mount_path = [\n")?;
    let vars = [
        ("HOME", workspace.home.as_str()),
        ("MRIYA_CONFIG_PATH", workspace.config.as_str()),
    ];

    let Err(err) = load_sync_with_env(&vars) else {
        bail!("a malformed file must not be ignored");
    };
    let message = err.to_string();

    ensure!(
        message.contains("TOML parse error"),
        "error should report the parse failure: {message}"
    );
    Ok(())
}

#[test]
fn directory_config_path_is_reported() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    let directory = workspace.root.join("mriya.toml");
    cap_std::fs_utf8::Dir::create_ambient_dir_all(&directory, ambient_authority())
        .context("create directory")?;
    let vars = [
        ("HOME", workspace.home.as_str()),
        ("MRIYA_CONFIG_PATH", workspace.config.as_str()),
    ];

    let Err(err) = load_sync_with_env(&vars) else {
        bail!("a directory in place of a file must not be ignored");
    };
    let message = err.to_string();

    ensure!(
        message.contains("not a regular file"),
        "error should explain the path is not a file: {message}"
    );
    Ok(())
}

#[test]
fn config_store_writes_volume_id_that_scaleway_config_reads() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write(
        "mriya.toml",
        "secret_key = \"secret\"\ndefault_project_id = \"project\"\n",
    )?;
    let store = store_for(&workspace);

    let written = store
        .write_volume_id("volume-123", true)
        .context("write volume id")?;
    ensure!(
        written.as_str() == workspace.config,
        "store should write the discovered file"
    );

    let config = load_scaleway(&workspace)?;
    ensure!(
        config.default_volume_id.as_deref() == Some("volume-123"),
        "the loader must read the id the store wrote, got {:?}",
        config.default_volume_id
    );
    Ok(())
}

#[test]
fn config_store_detects_legacy_sectioned_volume_id() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write(
        "mriya.toml",
        "[scaleway]\ndefault_volume_id = \"legacy-volume\"\n",
    )?;
    let store = store_for(&workspace);

    let volume_id = store
        .current_volume_id()
        .context("read the legacy volume id")?;

    ensure!(
        volume_id.as_deref() == Some("legacy-volume"),
        "the legacy section must still be detected, got {volume_id:?}"
    );
    Ok(())
}
