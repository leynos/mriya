//! Cloud-init user-data resolution utilities.
//!
//! Cloud-init user-data can be provided either inline (for example `#cloud-config`)
//! or via a file path. This module centralizes the branching and file loading
//! logic so CLI and configuration paths stay consistent over time.

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use thiserror::Error;

use crate::sync::{expand_tilde_with_home, home_from_env};

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
    /// Raised when a `~/` file path needs a `HOME` that is not valid UTF-8.
    ///
    /// An undecodable `HOME` is reported rather than quietly leaving the path
    /// unexpanded, because reading the literal path `~/…` would fail with an
    /// error that does not name the real cause.
    #[error(
        "cannot expand `{path}`: HOME is set to a value that is not valid UTF-8; set HOME to a UTF-8 directory or pass an absolute --cloud-init-file path"
    )]
    HomeUnavailable {
        /// File path that needed the expansion.
        path: String,
    },
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
    resolve_cloud_init_user_data_with_env(inline, file, &mockable::DefaultEnv)
}

/// Resolves cloud-init user-data, expanding `~/` through `env`.
///
/// Identical to [`resolve_cloud_init_user_data`] except that the `HOME`
/// lookup used for tilde expansion goes through the supplied environment.
///
/// # Errors
///
/// Returns [`CloudInitError`] when the inputs are invalid or the file cannot be
/// read.
pub fn resolve_cloud_init_user_data_with_env(
    inline: Option<&str>,
    file: Option<&str>,
    env: &impl mockable::Env,
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

    let expanded = expand_cloud_init_path(path, env)?;
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

/// Expands a `~/` cloud-init file path, blaming the path when `HOME` is bad.
///
/// [`home_from_env`] reports the environment failure without knowing which
/// path needed it; this wrapper adds that context so the CLI message names the
/// file the user asked for rather than only the variable.
///
/// # Errors
///
/// Returns [`CloudInitError::HomeUnavailable`] when `path` starts with `~/`
/// and `HOME` is set to a value that is not valid Unicode.
fn expand_cloud_init_path(path: &str, env: &impl mockable::Env) -> Result<String, CloudInitError> {
    let home = home_from_env(env, path).map_err(|_| CloudInitError::HomeUnavailable {
        path: path.to_owned(),
    })?;
    Ok(expand_tilde_with_home(path, home.as_deref()))
}

#[cfg(test)]
mod tests {
    //! Unit tests for environment-injected cloud-init resolution.

    use std::ffi::OsString;

    use super::*;
    use crate::test_helpers::{mock_env, mock_env_with_raw};

    #[test]
    fn tilde_file_path_expands_through_injected_home() {
        let tmp = tempfile::TempDir::new().expect("temporary home should be created");
        let home = Utf8Path::from_path(tmp.path()).expect("temporary home should be UTF-8");
        let dir = Dir::open_ambient_dir(home, ambient_authority()).expect("home should open");
        dir.write("user-data.txt", "payload")
            .expect("fixture should be written");

        let env = mock_env(&[("HOME", home.as_str())]);
        let resolved = resolve_cloud_init_user_data_with_env(None, Some("~/user-data.txt"), &env)
            .expect("tilde path should resolve");

        assert_eq!(resolved.as_deref(), Some("payload"));
    }

    #[test]
    fn tilde_file_path_reports_an_undecodable_home() {
        let env = mock_env_with_raw(|_| {
            Err(std::env::VarError::NotUnicode(OsString::from(
                "/home/\u{fffd}",
            )))
        });

        let err = resolve_cloud_init_user_data_with_env(None, Some("~/user-data.txt"), &env)
            .expect_err("an undecodable HOME should be reported");

        assert_eq!(
            err,
            CloudInitError::HomeUnavailable {
                path: String::from("~/user-data.txt")
            }
        );
    }

    #[test]
    fn absolute_file_path_ignores_an_undecodable_home() {
        // A path that needs no expansion must not fail because HOME could not
        // be decoded; the read itself is what decides the outcome.
        let env = mock_env_with_raw(|_| {
            Err(std::env::VarError::NotUnicode(OsString::from(
                "/home/\u{fffd}",
            )))
        });

        let err = resolve_cloud_init_user_data_with_env(
            None,
            Some("/nonexistent-dir/user-data.txt"),
            &env,
        )
        .expect_err("the missing file should be reported as a read failure");

        assert!(
            matches!(err, CloudInitError::FileRead { ref path, .. } if path.ends_with("user-data.txt")),
            "an absolute path should reach the file read: {err:?}"
        );
    }
}
