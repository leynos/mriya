//! Pointing the spelling engine at one fixture directory's own configuration.

use camino::Utf8Path;
use typos_cli::{config::Config, policy::ConfigEngine};

/// Points `engine` at the fixture directory's own configuration alone.
pub(crate) fn configure_engine(
    engine: &mut ConfigEngine<'_>,
    root: &Utf8Path,
) -> Result<(), String> {
    // Isolated mode stops the engine walking up to this worktree's own
    // configuration, so the fixture directory states the whole policy.
    engine.set_isolated(true);
    let overrides = Config::from_file(&root.join("typos.toml").into_std_path_buf())
        .map_err(|error| format!("typos.toml should parse: {error}"))?
        .ok_or_else(|| "typos.toml should carry a Typos configuration".to_owned())?;
    engine.set_overrides(overrides);
    engine
        .init_dir(root.as_std_path())
        .map_err(|error| format!("the work dir should initialize: {error}"))
}
