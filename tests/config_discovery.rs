//! Integration tests for configuration discovery and persisted volume IDs.
//!
//! These tests drive real discovery and the `ConfigStore` against a temporary
//! workspace. They pin how absent, malformed, and unreadable configuration
//! files are reported, and that the store writes a volume ID that
//! `ScalewayConfig` reads back.

mod config_support;

use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use camino::Utf8Path;
use cap_std::ambient_authority;
use config_support::{Workspace, path_text, shared_env};
use mriya::{ConfigStore, ConfigWriter, ScalewayConfig, SyncConfig};
use ortho_config::{ConfigDiscovery, MapEnv, OrthoConfig, SharedEnvSource};

/// Builds the map alone, for discovery sources that are supplied separately.
fn env_map(vars: &[(&str, &str)]) -> MapEnv {
    vars.iter().fold(MapEnv::new(), |map, (key, value)| {
        map.with_var(*key, *value)
    })
}

fn load_sync_with_env(workspace: &Workspace, vars: &[(&str, &str)]) -> anyhow::Result<SyncConfig> {
    let (discovery, merge) = shared_env(vars, &workspace.root);
    SyncConfig::load_from_iter_with_sources(["mriya"], discovery, merge)
        .map_err(|err| anyhow::anyhow!("sync configuration failed: {err}"))
}

fn load_scaleway(workspace: &Workspace) -> anyhow::Result<ScalewayConfig> {
    let vars = [
        ("HOME", workspace.home.as_str()),
        ("MRIYA_CONFIG_PATH", workspace.config.as_str()),
    ];
    let (discovery, merge) = shared_env(&vars, &workspace.root);
    ScalewayConfig::load_from_iter_with_sources(["mriya"], discovery, merge)
        .map_err(|err| anyhow::anyhow!("scaleway configuration failed: {err}"))
}

fn store_for(workspace: &Workspace) -> ConfigStore {
    let env = Arc::new(
        env_map(&[("MRIYA_CONFIG_PATH", workspace.config.as_str())])
            .with_current_dir(&workspace.root),
    );
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

/// Builds the environment layout a real run sees: the fixture home and
/// working directory, with the named extra variables.
fn layout_env<'a>(
    workspace: &'a Workspace,
    extra: &'a [(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    let mut vars = vec![
        ("HOME", workspace.home.as_str()),
        ("XDG_CONFIG_HOME", workspace.home.as_str()),
        ("XDG_CONFIG_DIRS", workspace.home.as_str()),
    ];
    vars.extend_from_slice(extra);
    vars
}

/// Builds a store whose discovery sees only the injected layout environment.
///
/// The working directory is injected too, so project-file discovery cannot
/// read the checkout this test runs in.
fn store_for_layout(workspace: &Workspace, vars: &[(&str, &str)]) -> ConfigStore {
    let env: SharedEnvSource = Arc::new(env_map(vars).with_current_dir(&workspace.root));
    let discovery = ConfigDiscovery::builder("mriya")
        .env_var("MRIYA_CONFIG_PATH")
        .config_file_name("mriya.toml")
        .dotfile_name(".mriya.toml")
        .project_file_name("mriya.toml")
        .env_source(env)
        .build();
    ConfigStore::with_discovery(discovery)
}

/// Loads `ScalewayConfig` through the same injected layout the store sees.
fn load_scaleway_with_layout(
    workspace: &Workspace,
    vars: &[(&str, &str)],
) -> anyhow::Result<ScalewayConfig> {
    let env = Arc::new(env_map(vars).with_current_dir(&workspace.root));
    ScalewayConfig::load_from_iter_with_sources(["mriya"], env.clone(), env)
        .map_err(|err| anyhow::anyhow!("scaleway configuration failed: {err}"))
}

/// Writes `contents` at an arbitrary fixture path, creating its parent.
fn write_at(path: &Utf8Path, contents: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("fixture path should have a parent directory")?;
    cap_std::fs_utf8::Dir::create_ambient_dir_all(parent, ambient_authority())
        .context("create fixture directory")?;
    let dir = cap_std::fs_utf8::Dir::open_ambient_dir(parent, ambient_authority())
        .context("open fixture directory")?;
    let name = path
        .file_name()
        .context("fixture path should have a file name")?;
    dir.write(name, contents)
        .with_context(|| format!("write {path}"))
}

/// Selecting an absent path suppresses project-file discovery, so the test
/// never reads a `mriya.toml` that happens to sit in the working directory.
#[test]
fn missing_selected_config_file_is_not_an_error() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    let missing = path_text(&workspace.root.join("missing.toml"));
    let vars = [
        ("HOME", workspace.home.as_str()),
        ("XDG_CONFIG_HOME", workspace.home.as_str()),
        ("XDG_CONFIG_DIRS", workspace.home.as_str()),
        ("MRIYA_CONFIG_PATH", missing.as_str()),
    ];

    let config = load_sync_with_env(&workspace, &vars)
        .context("an absent selected file should fall back to defaults")?;

    ensure!(config.volume_mount_path == mriya::sync::DEFAULT_VOLUME_MOUNT_PATH);
    Ok(())
}

/// With no explicit file selected, discovery falls back to the pinned
/// working directory. The decoy proves which directory that is: the fixture's,
/// not the checkout the tests run from.
#[test]
fn absent_explicit_path_reads_the_pinned_project_file() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    workspace.write("mriya.toml", "volume_mount_path = \"/decoy\"\n")?;
    let missing = path_text(&workspace.root.join("missing.toml"));
    let vars = [
        ("HOME", workspace.home.as_str()),
        ("XDG_CONFIG_HOME", workspace.home.as_str()),
        ("XDG_CONFIG_DIRS", workspace.home.as_str()),
        ("MRIYA_CONFIG_PATH", missing.as_str()),
    ];

    let config = load_sync_with_env(&workspace, &vars)?;

    ensure!(
        config.volume_mount_path == "/decoy",
        "the pinned project file should be read, got {}",
        config.volume_mount_path
    );
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

    let Err(err) = load_sync_with_env(&workspace, &vars) else {
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

    let Err(err) = load_sync_with_env(&workspace, &vars) else {
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

/// Writes the credentials a load needs into `path`.
fn seed_credentials_at(path: &Utf8Path) -> anyhow::Result<()> {
    write_at(
        path,
        "secret_key = \"secret\"\ndefault_project_id = \"project\"\n",
    )
}

/// Checks that `store` wrote exactly `expected`, and that the loader built
/// from the same injected layout reads the ID back with the seeded
/// credentials. This is the compatibility contract: the store and the loader
/// must choose the same file from the same environment.
fn assert_store_and_loader_agree(
    store: &ConfigStore,
    workspace: &Workspace,
    vars: &[(&str, &str)],
    expected: &Utf8Path,
) -> anyhow::Result<()> {
    let written = store
        .write_volume_id("volume-123", false)
        .context("write volume id")?;
    ensure!(
        written == expected,
        "store should write {expected}, wrote {written}"
    );

    let config = load_scaleway_with_layout(workspace, vars)?;
    ensure!(
        config.secret_key == "secret",
        "the loader must read the credentials from {expected}"
    );
    ensure!(
        config.default_volume_id.as_deref() == Some("volume-123"),
        "the loader must read the id the store wrote to {expected}, got {:?}",
        config.default_volume_id
    );
    Ok(())
}

#[test]
fn store_and_loader_agree_on_the_home_dotfile() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    let dotfile = workspace.root.join("home").join(".mriya.toml");
    seed_credentials_at(&dotfile)?;
    let vars = layout_env(&workspace, &[]);

    assert_store_and_loader_agree(
        &store_for_layout(&workspace, &vars),
        &workspace,
        &vars,
        &dotfile,
    )
}

#[test]
fn store_and_loader_agree_on_the_xdg_config_directory() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    let xdg_home = workspace.root.join("home");
    let xdg_file = xdg_home.join("mriya").join("mriya.toml");
    seed_credentials_at(&xdg_file)?;
    let extra = [("XDG_CONFIG_HOME", xdg_home.as_str())];
    let vars = layout_env(&workspace, &extra);

    assert_store_and_loader_agree(
        &store_for_layout(&workspace, &vars),
        &workspace,
        &vars,
        &xdg_file,
    )
}

#[test]
fn store_and_loader_agree_on_the_project_file() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    seed_credentials_at(&workspace.root.join("mriya.toml"))?;
    let vars = layout_env(&workspace, &[]);

    assert_store_and_loader_agree(
        &store_for_layout(&workspace, &vars),
        &workspace,
        &vars,
        &workspace.root.join("mriya.toml"),
    )
}

/// An absent explicit path is not an error: the store skips it and uses the
/// same file the loader reads.
#[test]
fn store_and_loader_agree_when_the_explicit_path_is_absent() -> anyhow::Result<()> {
    let workspace = Workspace::new()?;
    let missing = path_text(&workspace.root.join("missing.toml"));
    let project_file = workspace.root.join("mriya.toml");
    // The project file already holds the credentials; the store adds the
    // volume ID to it, so one file answers both discovery and loading.
    seed_credentials_at(&project_file)?;
    let mut vars = layout_env(&workspace, &[]);
    vars.push(("MRIYA_CONFIG_PATH", missing.as_str()));

    assert_store_and_loader_agree(
        &store_for_layout(&workspace, &vars),
        &workspace,
        &vars,
        &project_file,
    )
}
