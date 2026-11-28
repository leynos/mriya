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

#[cfg(test)]
mod tests {
    //! Unit and property tests for the volume-ID document shape.

    use anyhow::{Context as _, bail, ensure};
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    proptest! {
        #[test]
        fn write_then_read_round_trips_the_trimmed_value(
            leading in "[ \t]{0,4}",
            id in "[A-Za-z0-9_-]{0,24}",
            trailing in "[ \t]{0,4}",
        ) {
            let path = Utf8Path::new("mriya.toml");
            let raw = format!("{leading}{id}{trailing}");
            let mut document = toml::Value::Table(toml::value::Table::new());

            write_volume_id_value(path, &mut document, &raw)
                .expect("writing a string value should succeed");
            let read = read_volume_id(path, &document).expect("reading it back should succeed");

            let trimmed = raw.trim();
            if trimmed.is_empty() {
                prop_assert_eq!(read, None);
            } else {
                prop_assert_eq!(read.as_deref(), Some(trimmed));
            }
        }
    }

    /// Selects the volume ID a file records: the root-level key when it holds a
    /// value, otherwise a usable legacy `[scaleway]` value, and `None` when neither
    /// does. A blank root value records nothing, so it must not hide an ID an
    /// earlier release recorded.
    #[rstest]
    #[case::root_key_wins(
        "default_volume_id = \"root-volume\"\n[scaleway]\ndefault_volume_id = \"legacy-volume\"\n",
        Some("root-volume")
    )]
    #[case::legacy_section_is_detected(
        "[scaleway]\ndefault_volume_id = \"legacy-volume\"\n",
        Some("legacy-volume")
    )]
    #[case::blank_root_is_unset("default_volume_id = \"  \"\n", None)]
    #[case::blank_root_falls_back_to_legacy(
        "default_volume_id = \"\"\n[scaleway]\ndefault_volume_id = \"legacy-volume\"\n",
        Some("legacy-volume")
    )]
    fn read_volume_id_selects_the_recorded_volume(
        #[case] contents: &str,
        #[case] expected: Option<&str>,
    ) -> anyhow::Result<()> {
        let path = Utf8Path::new("mriya.toml");
        let value: toml::Value = toml::from_str(contents).context("config should parse")?;

        let volume_id = read_volume_id(path, &value).context("read volume id should succeed")?;

        ensure!(
            volume_id.as_deref() == expected,
            "expected {expected:?}, got {volume_id:?}"
        );
        Ok(())
    }

    /// One malformed document shape and the message it must produce.
    enum InvalidShape {
        /// A parsed TOML document.
        Document(&'static str),
        /// A document whose root is not a table at all.
        NonTableRoot,
    }

    /// Every malformed document shape is rejected with the structure error and a
    /// message naming the offending part.
    #[rstest]
    #[case::non_table_root(InvalidShape::NonTableRoot, "configuration root is not a table")]
    #[case::non_string_root_key(
        InvalidShape::Document("default_volume_id = 42"),
        "default_volume_id must be a string"
    )]
    #[case::non_table_section(
        InvalidShape::Document("scaleway = 42"),
        "[scaleway] must be a table"
    )]
    #[case::non_string_legacy_key(
        InvalidShape::Document("[scaleway]\ndefault_volume_id = 42"),
        "scaleway.default_volume_id must be a string"
    )]
    fn read_volume_id_rejects_invalid_shapes(
        #[case] shape: InvalidShape,
        #[case] expected: &str,
    ) -> anyhow::Result<()> {
        let fixture_path = Utf8Path::new("mriya.toml");
        let value = match shape {
            InvalidShape::Document(contents) => {
                toml::from_str(contents).context("fixture should parse")?
            }
            InvalidShape::NonTableRoot => toml::Value::Integer(42),
        };

        let Err(err) = read_volume_id(fixture_path, &value) else {
            bail!("read should fail for {expected:?}");
        };

        let ConfigStoreError::InvalidStructure { path, message } = err else {
            bail!("expected InvalidStructure, got {err:?}");
        };
        ensure!(path == fixture_path, "error should cite the config path");
        ensure!(
            message.contains(expected),
            "error should say {expected:?}, got {message:?}"
        );
        Ok(())
    }
}
