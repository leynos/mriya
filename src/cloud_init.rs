//! Cloud-init user-data resolution utilities.
//!
//! Cloud-init user-data can be provided either inline (for example `#cloud-config`)
//! or via a file path. This module centralises the branching and file loading
//! logic so CLI and configuration paths stay consistent over time.

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use thiserror::Error;

use crate::sync::expand_tilde;

/// Errors raised while resolving cloud-init user-data.
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum CloudInitError {
    /// Raised when both inline and file sources are provided.
    #[error("cloud-init user-data cannot be provided both inline and via file")]
    BothProvided,
    /// Raised when an inline payload is empty or only whitespace.
    #[error("cloud-init user-data must not be empty")]
    InlineEmpty,
    /// Raised when a file path is empty or only whitespace.
    #[error("cloud-init user-data file path must not be empty")]
    FilePathEmpty,
    /// Raised when a file resolves to empty or only whitespace.
    #[error("cloud-init user-data file must not be empty")]
    FileEmpty,
    /// Raised when reading the file source fails.
    #[error("failed to read cloud-init user-data file `{path}`: {message}")]
    FileRead {
        /// Expanded path that failed to read.
        path: String,
        /// Underlying error message.
        message: String,
    },
}

/// Resolves cloud-init user-data from either an inline value or a file.
///
/// Inline and file sources are mutually exclusive. Both values are trimmed for
/// emptiness checks, but the returned payload preserves the original content.
///
/// # Errors
///
/// Returns [`CloudInitError`] when the inputs are invalid or the file cannot be
/// read.
pub fn resolve_cloud_init_user_data(
    inline: Option<&str>,
    file: Option<&str>,
) -> Result<Option<String>, CloudInitError> {
    if inline.is_some() && file.is_some() {
        return Err(CloudInitError::BothProvided);
    }

    if let Some(payload) = inline {
        validate_payload(payload)?;
        return Ok(Some(payload.to_owned()));
    }

    let Some(path) = file else {
        return Ok(None);
    };

    if path.trim().is_empty() {
        return Err(CloudInitError::FilePathEmpty);
    }

    let expanded = expand_tilde(path);
    let content =
        read_to_string_ambient(&expanded).map_err(|message| CloudInitError::FileRead {
            path: expanded.clone(),
            message,
        })?;

    validate_payload(&content).map_err(|err| match err {
        CloudInitError::InlineEmpty => CloudInitError::FileEmpty,
        other => other,
    })?;

    Ok(Some(content))
}

/// Validates that a user-data payload is not empty/whitespace.
pub(crate) fn validate_payload(payload: &str) -> Result<(), CloudInitError> {
    if payload.trim().is_empty() {
        return Err(CloudInitError::InlineEmpty);
    }
    Ok(())
}

fn read_to_string_ambient(path: &str) -> Result<String, String> {
    let path_buf = Utf8Path::new(path);

    let (dir_path, file_path) = if path_buf.is_absolute() {
        let parent = path_buf
            .parent()
            .ok_or_else(|| format!("path has no parent directory: {path_buf}"))?;
        let file_name = path_buf
            .file_name()
            .ok_or_else(|| format!("path has no file name: {path_buf}"))?;
        (parent, Utf8Path::new(file_name))
    } else {
        (Utf8Path::new("."), path_buf)
    };

    let dir =
        Dir::open_ambient_dir(dir_path, ambient_authority()).map_err(|err| err.to_string())?;
    dir.read_to_string(file_path).map_err(|err| err.to_string())
}
