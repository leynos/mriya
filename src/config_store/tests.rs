//! Tests for configuration store helpers.
//!
//! The configuration store handles discovery of `mriya.toml`, reads and parses
//! its content, and writes the `volume_id` entry used to mount cache volumes.
//! These tests cover creation and overwrite flows, plus error paths such as
//! invalid TOML and missing parent directories. Temporary directories and
//! fixtures keep each case isolated from repo configuration.

use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use ortho_config::MapEnv;
use rstest::{fixture, rstest};
use tempfile::TempDir;

use super::*;

struct ConfigFixture {
    _tmp: TempDir,
    path: Utf8PathBuf,
    store: ConfigStore,
}

#[fixture]
fn config_fixture() -> anyhow::Result<ConfigFixture> {
    let tmp = TempDir::new().context("failed to create tempdir")?;
    let path = temp_config_path(&tmp)?;
    let store = ConfigStore::with_discovery(discovery_for_path(&path)?);
    Ok(ConfigFixture {
        _tmp: tmp,
        path,
        store,
    })
}

/// Builds discovery that sees only the fixture directory.
///
/// The environment is injected rather than read from the process, so a
/// developer's `MRIYA_CONFIG_PATH` or platform configuration directory cannot
/// redirect a `write_volume_id` call onto a real file. Selecting the fixture
/// path explicitly also keeps project-file discovery from walking upward.
fn discovery_for_path(path: &Utf8Path) -> anyhow::Result<ConfigDiscovery> {
    let root = path
        .parent()
        .context("temp path should have a parent directory")?;
    // No explicit-path variable is injected: leaving it unset lets the
    // fixture's project root be the final candidate, which is where the store
    // falls back when no file exists yet. HOME and the XDG directories point
    // at the fixture so a developer's real configuration cannot be read.
    let env = Arc::new(
        MapEnv::new()
            .with_var("HOME", root.as_str())
            .with_var("XDG_CONFIG_HOME", root.as_str())
            .with_var("XDG_CONFIG_DIRS", root.as_str()),
    );
    Ok(ConfigDiscovery::builder(APP_NAME)
        .env_var(CONFIG_ENV_VAR)
        .config_file_name(CONFIG_FILE_NAME)
        .dotfile_name(DOTFILE_NAME)
        .project_file_name(PROJECT_FILE_NAME)
        .env_source(env)
        .clear_project_roots()
        .add_project_root(root)
        .build())
}

fn temp_config_path(tmp: &TempDir) -> anyhow::Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(tmp.path().join("mriya.toml"))
        .map_err(|path| anyhow::anyhow!("temp path should be utf8: {}", path.display()))
}

#[rstest]
fn write_volume_id_creates_config_file(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    // Bind the guard so the temporary directory outlives the writes below.
    let ConfigFixture { _tmp, path, store } = config_fixture?;

    let written_path = store
        .write_volume_id("vol-123", true)
        .context("write volume id should succeed")?;

    ensure!(written_path == path, "written path should match fixture");
    let contents = read_config(&path).context("read config should succeed")?;
    let value = parse_toml(&path, &contents).context("parse config should succeed")?;
    let volume_id = read_volume_id(&path, &value).context("extract volume id should succeed")?;
    ensure!(
        volume_id == Some(String::from("vol-123")),
        "volume id should round-trip"
    );
    Ok(())
}

/// Confirms the store writes the root-level key that `ortho_config` reads,
/// rather than the `[scaleway]` section earlier releases used.
#[rstest]
fn write_volume_id_uses_root_level_key(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    // Bind the guard so the temporary directory outlives the writes below.
    let ConfigFixture { _tmp, path, store } = config_fixture?;

    store
        .write_volume_id("vol-123", true)
        .context("write volume id should succeed")?;

    let contents = read_config(&path).context("read config should succeed")?;
    let value = parse_toml(&path, &contents).context("parse config should succeed")?;
    let table = value
        .as_table()
        .context("configuration root should be a table")?;
    ensure!(
        table.get(VOLUME_KEY).and_then(toml::Value::as_str) == Some("vol-123"),
        "volume id should be a root-level key"
    );
    ensure!(
        !table.contains_key(LEGACY_SCALEWAY_SECTION),
        "the store must not write the legacy section"
    );
    Ok(())
}

/// Builds a store whose discovery selects `name` inside `root`, with the
/// fixture root as the only working directory and XDG location.
fn store_selecting(root: &Utf8Path, name: &str) -> ConfigStore {
    let selected = root.join(name);
    let env = Arc::new(
        MapEnv::new()
            .with_var(CONFIG_ENV_VAR, selected.as_str())
            .with_current_dir(root)
            .with_var("HOME", root.as_str())
            .with_var("XDG_CONFIG_HOME", root.as_str())
            .with_var("XDG_CONFIG_DIRS", root.as_str()),
    );
    let discovery = ConfigDiscovery::builder(APP_NAME)
        .env_var(CONFIG_ENV_VAR)
        .config_file_name(CONFIG_FILE_NAME)
        .dotfile_name(DOTFILE_NAME)
        .project_file_name(PROJECT_FILE_NAME)
        .env_source(env)
        .clear_project_roots()
        .add_project_root(root)
        .build();
    ConfigStore::with_discovery(discovery)
}

/// The loader parses only these extensions as something other than TOML, so a
/// store that refused more than this would reject a file `run` accepts.
#[rstest]
#[case::yaml("mriya.yaml", "default_volume_id: legacy\n", Some("yaml"))]
#[case::json5("mriya.json5", "{ default_volume_id: 'legacy' }\n", Some("json5"))]
#[case::named_toml("config.conf", "default_volume_id = \"legacy\"\n", None)]
#[case::extensionless("mriya-config", "default_volume_id = \"legacy\"\n", None)]
fn write_volume_id_matches_the_loader_format_classification(
    #[case] name: &str,
    #[case] contents: &str,
    #[case] refused_format: Option<&str>,
) -> anyhow::Result<()> {
    let tmp = TempDir::new().context("failed to create tempdir")?;
    let root = Utf8Path::from_path(tmp.path()).context("temp path should be UTF-8")?;
    Dir::open_ambient_dir(root, ambient_authority())
        .context("temp directory should open")?
        .write(name, contents)
        .with_context(|| format!("write {name}"))?;
    let store = store_selecting(root, name);

    let result = store.write_volume_id("vol-123", true);

    if let Some(expected) = refused_format {
        let Err(err) = result else {
            bail!("{name} must not be edited as TOML");
        };
        let ConfigStoreError::UnsupportedFormat { path, format } = err else {
            bail!("expected UnsupportedFormat, got {err:?}");
        };
        ensure!(path == root.join(name), "error should cite {name}");
        ensure!(
            format == expected,
            "error should name {expected}, got {format}"
        );
    } else {
        let written = result.with_context(|| format!("{name} is TOML and should be edited"))?;
        ensure!(written == root.join(name), "store should write {name}");
    }
    Ok(())
}

#[rstest]
fn write_volume_id_rejects_existing_without_force(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    let fixture = config_fixture?;
    fixture
        .store
        .write_volume_id("vol-123", true)
        .context("seed config should succeed")?;

    let Err(err) = fixture.store.write_volume_id("vol-456", false) else {
        bail!("overwrite should fail without force");
    };

    let ConfigStoreError::VolumeAlreadyConfigured { volume_id } = err else {
        bail!("expected VolumeAlreadyConfigured error");
    };
    ensure!(volume_id == "vol-123", "error should report existing id");
    Ok(())
}

#[rstest]
fn write_volume_id_overwrites_when_forced(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    let fixture = config_fixture?;
    fixture
        .store
        .write_volume_id("vol-123", true)
        .context("seed config should succeed")?;

    fixture
        .store
        .write_volume_id("vol-456", true)
        .context("overwrite config should succeed")?;

    let contents = read_config(&fixture.path).context("read config should succeed")?;
    let value = parse_toml(&fixture.path, &contents).context("parse config should succeed")?;
    let volume_id =
        read_volume_id(&fixture.path, &value).context("extract volume id should succeed")?;
    ensure!(
        volume_id == Some(String::from("vol-456")),
        "forced overwrite should replace the volume id"
    );
    Ok(())
}

#[rstest]
#[case("not = [")]
#[case("scaleway =")]
fn parse_toml_rejects_invalid_content(
    config_fixture: anyhow::Result<ConfigFixture>,
    #[case] contents: &str,
) -> anyhow::Result<()> {
    let fixture = config_fixture?;
    let Err(err) = parse_toml(&fixture.path, contents) else {
        bail!("parse should fail");
    };
    let ConfigStoreError::Parse { path, .. } = err else {
        bail!("expected parse error");
    };
    ensure!(path == fixture.path, "error should cite the config path");
    Ok(())
}

#[rstest]
fn read_config_reports_missing_parent_dir(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    let fixture = config_fixture?;
    let parent = fixture
        .path
        .parent()
        .context("config path should have a parent directory")?;
    let missing_path = parent.join("missing").join("nested").join("mriya.toml");

    let Err(err) = read_config(&missing_path) else {
        bail!("read should fail");
    };
    let ConfigStoreError::Io { path, .. } = err else {
        bail!("expected io error");
    };
    let missing_parent = missing_path
        .parent()
        .context("missing path should have a parent directory")?;
    ensure!(
        path == missing_parent.to_path_buf(),
        "error should cite the missing parent directory"
    );
    Ok(())
}

// Kills the `current_volume_id` and `path_exists` survivors tracked in #55.
/// Confirms a persisted volume ID is read back exactly by the production store.
#[rstest]
fn current_volume_id_round_trips_written_id(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    let fixture = config_fixture?;
    fixture
        .store
        .write_volume_id("vol-123", false)
        .context("write volume id should succeed")?;

    let volume_id = fixture
        .store
        .current_volume_id()
        .context("read volume id should succeed")?;
    ensure!(
        volume_id.as_deref() == Some("vol-123"),
        "expected the exact written id, got {volume_id:?}"
    );
    Ok(())
}

/// Confirms a missing config remains a successful `None` result.
#[rstest]
fn current_volume_id_returns_none_when_config_missing(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    let fixture = config_fixture?;
    let volume_id = fixture
        .store
        .current_volume_id()
        .context("read of a missing config should succeed")?;
    ensure!(
        volume_id.is_none(),
        "expected None without a config file, got {volume_id:?}"
    );
    Ok(())
}

/// Checks that probing through a regular-file parent preserves the I/O error.
#[rstest]
fn path_exists_reports_non_directory_parent(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    let fixture = config_fixture?;
    // Seed a regular file, then probe a path that uses it as a parent
    // directory: opening it fails with NotADirectory, which must surface
    // as an Io error rather than being swallowed as "does not exist".
    fixture
        .store
        .write_volume_id("vol-123", true)
        .context("seed config should succeed")?;
    let nested = fixture.path.join("mriya.toml");

    let Err(err) = path_exists(&nested) else {
        bail!("probe through a file should fail");
    };
    let ConfigStoreError::Io { path, .. } = err else {
        bail!("expected io error, got {err:?}");
    };
    ensure!(
        path == fixture.path,
        "error should cite the non-directory parent"
    );
    Ok(())
}
