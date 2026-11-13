//! Shared hermetic fixtures for the layered-configuration integration tests.
//!
//! Each integration test file is its own crate, so this module holds only the
//! fixtures that every file uses. Keeping them here avoids copying the
//! temporary-workspace setup while leaving each test crate free of unused
//! helpers.

use std::sync::Arc;

use anyhow::Context as _;
use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs_utf8::Dir};
use ortho_config::{MapEnv, SharedEnvSource, SharedScanEnvSource};
use tempfile::TempDir;

/// A hermetic workspace with a home directory and a target config path.
pub struct Workspace {
    _tmp: TempDir,
    pub root: Utf8PathBuf,
    pub home: String,
    pub config: String,
}

impl Workspace {
    pub fn new() -> anyhow::Result<Self> {
        let tmp = TempDir::new().context("create tempdir")?;
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf())
            .map_err(|path| anyhow::anyhow!("temp path is not utf8: {}", path.display()))?;
        let home_path = root.join("home");
        Dir::create_ambient_dir_all(&home_path, ambient_authority()).context("create home")?;
        let home = path_text(&home_path);
        let config = path_text(&root.join("mriya.toml"));
        Ok(Self {
            _tmp: tmp,
            root,
            home,
            config,
        })
    }

    pub fn write(&self, name: &str, contents: &str) -> anyhow::Result<()> {
        let dir = Dir::open_ambient_dir(&self.root, ambient_authority()).context("open root")?;
        dir.write(name, contents)
            .with_context(|| format!("write {name}"))
    }
}

pub fn path_text(path: &camino::Utf8Path) -> String {
    path.as_str().to_owned()
}

/// Builds one map, shared as both the discovery lookup and the merge scanner.
pub fn shared_env(vars: &[(&str, &str)]) -> (SharedEnvSource, SharedScanEnvSource) {
    let mut map = MapEnv::new();
    for (key, value) in vars {
        map = map.with_var(*key, *value);
    }
    let shared = Arc::new(map);
    (shared.clone(), shared)
}
