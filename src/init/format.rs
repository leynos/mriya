//! Failure rendering for the `mkfs.ext4` format command.

use std::fmt;

use crate::sync::{RemoteCommandOutput, SyncError};

/// Why formatting failed, kept so the error can carry its cause.
#[derive(Debug)]
pub(super) struct FormatFailure {
    /// Description of the failure, shown to the user.
    pub(super) message: String,
    /// Underlying SSH error, absent when the command ran but exited non-zero.
    pub(super) source: Option<SyncError>,
}

impl fmt::Display for FormatFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Builds the failure for a format command that could not be started.
///
/// `write_error` prints only the outermost `Display` and no cause chain, so the
/// cause and its remedy have to be in the message itself. A stored source
/// alone would report "failed to execute format command", and a user whose
/// `HOME` cannot be decoded would look for a broken volume instead.
pub(super) fn format_spawn_failure(err: SyncError) -> FormatFailure {
    FormatFailure {
        message: format!("failed to execute format command: {err}"),
        source: Some(err),
    }
}

/// Describes a non-zero or missing exit status from `mkfs.ext4`, including
/// trimmed stderr when there is any.
pub(super) fn format_failure_message(output: &RemoteCommandOutput) -> String {
    let stderr = output.stderr.trim();
    match output.exit_code {
        Some(code) if stderr.is_empty() => format!("mkfs.ext4 exited with status {code}"),
        Some(code) => format!("mkfs.ext4 exited with status {code}: {stderr}"),
        None if stderr.is_empty() => String::from("mkfs.ext4 terminated without an exit status"),
        None => format!("mkfs.ext4 terminated without an exit status: {stderr}"),
    }
}
