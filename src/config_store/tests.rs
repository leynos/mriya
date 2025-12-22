//! Tests for configuration store helpers.

use super::*;
use rstest::{fixture, rstest};
use tempfile::TempDir;

struct ConfigFixture {
    _tmp: TempDir,
    path: Utf8PathBuf,
    store: ConfigStore,
}

#[fixture]
fn config_fixture() -> ConfigFixture {
    let tmp = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
    let path = temp_config_path(&tmp);
    let store = ConfigStore::with_discovery(discovery_for_path(&path));
    ConfigFixture {
        _tmp: tmp,
        path,
        store,
    }
}

fn discovery_for_path(path: &Utf8Path) -> ConfigDiscovery {
    let root = path
        .parent()
        .expect("temp path should have a parent directory");
    ConfigDiscovery::builder(APP_NAME)
        .env_var(CONFIG_ENV_VAR)
        .config_file_name(CONFIG_FILE_NAME)
        .dotfile_name(DOTFILE_NAME)
        .project_file_name(PROJECT_FILE_NAME)
        .clear_project_roots()
        .add_project_root(root)
        .build()
}

fn temp_config_path(tmp: &TempDir) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(tmp.path().join("mriya.toml"))
        .unwrap_or_else(|err| panic!("temp path should be utf8: {}", err.display()))
}

#[rstest]
fn write_volume_id_creates_config_file(config_fixture: ConfigFixture) {
    let ConfigFixture { path, store, .. } = config_fixture;

    let written_path = store
        .write_volume_id("vol-123", true)
        .unwrap_or_else(|err| panic!("write volume id: {err}"));

    assert_eq!(written_path, path);
    let contents = read_config(&path).unwrap_or_else(|err| panic!("read config: {err}"));
    let value = parse_toml(&path, &contents).unwrap_or_else(|err| panic!("parse config: {err}"));
    let volume_id =
        read_volume_id(&path, &value).unwrap_or_else(|err| panic!("extract volume id: {err}"));
    assert_eq!(volume_id, Some(String::from("vol-123")));
}

#[rstest]
fn write_volume_id_rejects_existing_without_force(config_fixture: ConfigFixture) {
    config_fixture
        .store
        .write_volume_id("vol-123", true)
        .unwrap_or_else(|err| panic!("seed config: {err}"));

    let Err(err) = config_fixture.store.write_volume_id("vol-456", false) else {
        panic!("overwrite should fail without force");
    };

    let ConfigStoreError::VolumeAlreadyConfigured { volume_id } = err else {
        panic!("expected VolumeAlreadyConfigured error");
    };
    assert_eq!(volume_id, "vol-123");
}

#[rstest]
fn write_volume_id_overwrites_when_forced(config_fixture: ConfigFixture) {
    config_fixture
        .store
        .write_volume_id("vol-123", true)
        .unwrap_or_else(|err| panic!("seed config: {err}"));

    config_fixture
        .store
        .write_volume_id("vol-456", true)
        .unwrap_or_else(|err| panic!("overwrite config: {err}"));

    let contents =
        read_config(&config_fixture.path).unwrap_or_else(|err| panic!("read config: {err}"));
    let value = parse_toml(&config_fixture.path, &contents)
        .unwrap_or_else(|err| panic!("parse config: {err}"));
    let volume_id = read_volume_id(&config_fixture.path, &value)
        .unwrap_or_else(|err| panic!("extract volume id: {err}"));
    assert_eq!(volume_id, Some(String::from("vol-456")));
}

#[rstest]
#[case("not = [")]
#[case("scaleway =")]
fn parse_toml_rejects_invalid_content(config_fixture: ConfigFixture, #[case] contents: &str) {
    let Err(err) = parse_toml(&config_fixture.path, contents) else {
        panic!("parse should fail");
    };
    let ConfigStoreError::Parse { path, .. } = err else {
        panic!("expected parse error");
    };
    assert_eq!(path, config_fixture.path);
}

#[rstest]
fn read_config_reports_missing_parent_dir(config_fixture: ConfigFixture) {
    let missing_path = config_fixture
        .path
        .parent()
        .unwrap_or_else(|| Utf8Path::new("."))
        .join("missing")
        .join("nested")
        .join("mriya.toml");

    let Err(err) = read_config(&missing_path) else {
        panic!("read should fail");
    };
    let ConfigStoreError::Io { path, .. } = err else {
        panic!("expected io error");
    };
    assert_eq!(
        path,
        missing_path
            .parent()
            .unwrap_or_else(|| Utf8Path::new("."))
            .to_path_buf()
    );
}

#[rstest]
fn read_volume_id_rejects_non_table_root(config_fixture: ConfigFixture) {
    let Err(err) = read_volume_id(&config_fixture.path, &toml::Value::String("nope".into())) else {
        panic!("read should fail");
    };
    let ConfigStoreError::InvalidStructure { path, .. } = err else {
        panic!("expected invalid structure error");
    };
    assert_eq!(path, config_fixture.path);
}
