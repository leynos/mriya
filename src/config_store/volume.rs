//! Reading and writing the cache volume ID inside a parsed TOML document.
//!
//! The loader reads configuration files as root-level keys, so that is where
//! this module writes the ID. Earlier releases wrote it under a `[scaleway]`
//! table, which the loader never read; a read still detects that shape so
//! `mriya init` keeps refusing to replace an ID an earlier run recorded.

use camino::Utf8Path;
use ortho_config::toml;

use super::ConfigStoreError;

/// Root-level key read by `ortho_config`'s file layer.
pub(crate) const VOLUME_KEY: &str = "default_volume_id";

/// Section written by earlier releases; read only for compatibility.
pub(crate) const LEGACY_SCALEWAY_SECTION: &str = "scaleway";

/// Extracts the root-level `default_volume_id`, trimmed, from the parsed
/// document, and still detects a legacy `scaleway.default_volume_id`.
///
/// A usable root-level value wins. A blank root-level value records nothing,
/// so the legacy section is still consulted rather than hidden by it.
///
/// Returns `None` when neither key holds a value.
///
/// # Errors
///
/// Returns `InvalidStructure` when the root or legacy section is not a table,
/// or a present key is not a string.
pub(super) fn read_volume_id(
    path: &Utf8Path,
    value: &toml::Value,
) -> Result<Option<String>, ConfigStoreError> {
    let table = root_table(path, value)?;

    if let Some(raw) = table.get(VOLUME_KEY) {
        let root = parse_volume_id(path, raw, VOLUME_KEY)?;
        if root.is_some() {
            return Ok(root);
        }
        // A blank root value records nothing, but the legacy section may still
        // hold an ID an earlier release recorded, so keep guarding that one.
    }

    let Some(section) = table.get(LEGACY_SCALEWAY_SECTION) else {
        return Ok(None);
    };
    let section_table = section
        .as_table()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: format!("[{LEGACY_SCALEWAY_SECTION}] must be a table"),
        })?;

    section_table.get(VOLUME_KEY).map_or(Ok(None), |raw| {
        parse_volume_id(
            path,
            raw,
            &format!("{LEGACY_SCALEWAY_SECTION}.{VOLUME_KEY}"),
        )
    })
}

/// Sets the root-level `default_volume_id` (trimmed) in `value`, leaving other
/// keys alone.
///
/// # Errors
///
/// Returns `InvalidStructure` when the root is not a table.
pub(super) fn write_volume_id_value(
    path: &Utf8Path,
    value: &mut toml::Value,
    volume_id: &str,
) -> Result<(), ConfigStoreError> {
    let table = value
        .as_table_mut()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: String::from("configuration root is not a table"),
        })?;

    table.insert(
        String::from(VOLUME_KEY),
        toml::Value::String(volume_id.trim().to_owned()),
    );
    Ok(())
}

/// Borrows the document root as a table, naming `path` when it is not one.
fn root_table<'a>(
    path: &Utf8Path,
    value: &'a toml::Value,
) -> Result<&'a toml::value::Table, ConfigStoreError> {
    value
        .as_table()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: String::from("configuration root is not a table"),
        })
}

/// Reads one key as a trimmed volume ID, reporting its label in an error.
fn parse_volume_id(
    path: &Utf8Path,
    raw: &toml::Value,
    label: &str,
) -> Result<Option<String>, ConfigStoreError> {
    let text = raw
        .as_str()
        .ok_or_else(|| ConfigStoreError::InvalidStructure {
            path: path.to_path_buf(),
            message: format!("{label} must be a string"),
        })?;
    let trimmed = text.trim();
    // An empty value records no usable volume, so `mriya init` may set one
    // without `--force`.
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(trimmed.to_owned()))
}
