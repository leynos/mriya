//! Tests for configuration store helpers.
//!
//! The configuration store handles discovery of `mriya.toml`, reads and parses
//! its content, and writes the `volume_id` entry used to mount cache volumes.
//! These tests cover creation and overwrite flows, plus error paths such as
//! invalid TOML and missing parent directories. Temporary directories and
//! fixtures keep each case isolated from repo configuration.

use anyhow::{Context as _, bail, ensure};
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

fn discovery_for_path(path: &Utf8Path) -> anyhow::Result<ConfigDiscovery> {
    let root = path
        .parent()
        .context("temp path should have a parent directory")?;
    Ok(ConfigDiscovery::builder(APP_NAME)
        .env_var(CONFIG_ENV_VAR)
        .config_file_name(CONFIG_FILE_NAME)
        .dotfile_name(DOTFILE_NAME)
        .project_file_name(PROJECT_FILE_NAME)
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
    let ConfigFixture { path, store, .. } = config_fixture?;

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

#[rstest]
fn read_volume_id_rejects_non_table_root(
    config_fixture: anyhow::Result<ConfigFixture>,
) -> anyhow::Result<()> {
    let fixture = config_fixture?;
    let Err(err) = read_volume_id(&fixture.path, &toml::Value::String("nope".into())) else {
        bail!("read should fail");
    };
    let ConfigStoreError::InvalidStructure { path, .. } = err else {
        bail!("expected invalid structure error");
    };
    ensure!(path == fixture.path, "error should cite the config path");
    Ok(())
}
