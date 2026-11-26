//! Typed failures produced while formatting a new cache volume.

use super::error::InitError;
use crate::{
    cache_device::{CacheDeviceReport, CacheDeviceStatus},
    sync::SyncError,
};

/// Builds the failure for a format command that could not be started.
///
/// `write_error` prints only the outermost `Display` and no cause chain, so the
/// cause and its remedy have to be in the message itself. A stored source
/// alone would report "failed to execute format command", and a user whose
/// `HOME` cannot be decoded would look for a broken volume instead.
pub(crate) fn format_spawn_failure(err: SyncError) -> FormatFailure {
    FormatFailure::Spawn {
        message: format!("failed to execute format command: {err}"),
        source: err,
    }
}

/// Why formatting failed, kept so the error can carry its cause.
#[derive(Debug)]
pub(crate) enum FormatFailure {
    /// The SSH command could not be started.
    Spawn {
        /// Description that names the underlying cause and remedy.
        message: String,
        /// Underlying SSH error.
        source: SyncError,
    },
    /// The script ran but printed no usable status line.
    Protocol {
        /// Description of the missing status.
        message: String,
    },
    /// The script reported a refusal.
    Report(CacheDeviceReport),
}

/// Maps a formatting failure to the matching typed init error.
pub(crate) fn format_error<BackendError: std::error::Error + 'static>(
    volume_id: &str,
    failure: FormatFailure,
) -> InitError<BackendError> {
    match failure {
        FormatFailure::Spawn { message, source } => InitError::FormatFailed {
            volume_id: volume_id.to_owned(),
            message,
            source: Some(Box::new(source)),
        },
        FormatFailure::Protocol { message } => InitError::FormatFailed {
            volume_id: volume_id.to_owned(),
            message,
            source: None,
        },
        FormatFailure::Report(report) => {
            let message = report.describe();
            match report.status() {
                CacheDeviceStatus::NotFound => InitError::FormatDeviceNotFound {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                CacheDeviceStatus::Ambiguous => InitError::FormatDeviceAmbiguous {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                CacheDeviceStatus::RootDisk => InitError::FormatRootDisk {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                CacheDeviceStatus::RootUnknown => InitError::FormatRootUnknown {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                CacheDeviceStatus::SignaturePresent => InitError::FormatSignaturePresent {
                    volume_id: volume_id.to_owned(),
                    message,
                },
                _ => InitError::FormatFailed {
                    volume_id: volume_id.to_owned(),
                    message,
                    source: None,
                },
            }
        }
    }
}
