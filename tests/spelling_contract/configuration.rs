//! The effective mask list, read from the generated configuration.
//!
//! `typos.toml` is generated, so this is the only place that knows how to read
//! it. Both the model checks and the real-engine checks go through here, so
//! neither can judge a configuration the gate would not use.

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};

use crate::{CONFIG, OVERLAY};

/// The effective mask list, read out of the generated configuration.
///
/// Both files are TOML and the shared base writes multi-line patterns, so the
/// list is read as data rather than pattern-matched out of the file. A file that
/// does not parse, or that carries no list, is an error: an unreadable
/// configuration must never read as "there are no exemptions".
pub fn masks() -> Result<Vec<String>, String> {
    parses(CONFIG)?
        .get("default")
        .and_then(|default| default.get("extend-ignore-re"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "typos.toml should carry default.extend-ignore-re".to_owned())?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("an ignore-re entry should be a string: {entry}"))
        })
        .collect()
}

/// The patterns the overlay contributes, read as TOML rather than as lines.
///
/// Parsing is what makes this a check on the overlay: a hand-rolled scan for
/// one quote style would silently skip every entry written another way, so a
/// widened entry could reach the generated file with nothing comparing it. The
/// overlay is read with the same parser the gate's merge uses, so both quote
/// styles and any multi-line entry are covered.
pub fn overlay_patterns() -> Result<Vec<String>, String> {
    parses(OVERLAY)?
        .get("patterns")
        .and_then(|patterns| patterns.get("ignore"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "typos.local.toml should carry patterns.ignore".to_owned())?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("a patterns.ignore entry should be a string: {entry}"))
        })
        .collect()
}

/// Parses one configuration document, naming it in the error.
fn parses(document: &str) -> Result<toml::Table, String> {
    document
        .parse()
        .map_err(|error: toml::de::Error| format!("a configuration should parse: {error}"))
}

/// `typos.toml` is generated, so a stale copy would leave these tests judging
/// a configuration the gate never uses. Every overlay pattern must reach it.
#[test]
fn the_overlay_reaches_the_generated_configuration() -> Result<(), String> {
    let generated = masks()?;
    let missing: Vec<String> = overlay_patterns()?
        .into_iter()
        .filter(|entry| !generated.contains(entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "typos.toml lacks overlay entries {missing:?}; run make spelling"
    ))
}

/// The configuration files, paired with the copies compiled into this test.
const CONFIGURATIONS: [(&str, &str); 2] = [("typos.toml", CONFIG), ("typos.local.toml", OVERLAY)];

/// Reads one configuration file through a capability handle rooted at the
/// manifest directory.
fn read_configuration(name: &str) -> Result<String, String> {
    let manifest = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Dir::open_ambient_dir(manifest, ambient_authority())
        .map_err(|error| format!("{manifest} should open: {error}"))?;
    dir.read_to_string(name)
        .map_err(|error| format!("{manifest}/{name} should read: {error}"))
}

/// The configuration files on disk must be the ones compiled into this test, so
/// a failure above always names the file a contributor should regenerate.
#[test]
fn the_configuration_files_are_the_ones_the_gate_reads() -> Result<(), String> {
    let drifted: Vec<&str> = CONFIGURATIONS
        .into_iter()
        .filter_map(|(name, built_in)| {
            let matches = read_configuration(name).is_ok_and(|on_disk| on_disk == built_in);
            (!matches).then_some(name)
        })
        .collect();
    if drifted.is_empty() {
        return Ok(());
    }
    Err(format!(
        "these files differ from the tested copies: {drifted:?}"
    ))
}
