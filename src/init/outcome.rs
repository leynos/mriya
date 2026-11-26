//! Messages and wrappers shared by the init outcomes.
//!
//! Every outcome keeps the failure that caused it and describes what happened
//! to the new volume. These helpers build that description, and decide whether
//! the recovery journal can be removed after a verified cleanup.

use camino::Utf8PathBuf;

use super::InitError;
use crate::{cleanup::CleanupFailure, ownership::ResourceLedger, volume::VolumeHandle};

/// Configuration key that holds the cache volume identifier.
const VOLUME_KEY: &str = "default_volume_id";
/// Configuration section that holds the cache volume identifier.
const SCALEWAY_SECTION: &str = "scaleway";
/// Environment variable that overrides the cache volume identifier.
const VOLUME_ENV_VAR: &str = "SCW_DEFAULT_VOLUME_ID";

/// Removes the journal after verified cleanup, noting a failure.
///
/// Returns the journal path when it remains on disk, so the message can name
/// the file `mriya recover` should replay.
pub(super) fn clear_journal(
    ledger: &ResourceLedger,
    warnings: &mut Vec<String>,
) -> Option<Utf8PathBuf> {
    if ledger.has_unknown_outcome() {
        warnings.push(String::from(
            "an allocation outcome is unknown; the recovery journal is kept",
        ));
        return ledger.journal_path();
    }
    if let Err(error) = ledger.clear_journal() {
        warnings.push(format!(
            "could not remove the recovery journal: {message}",
            message = error.message
        ));
        return ledger.journal_path();
    }
    None
}

/// Wraps `primary` in the volume context `note`.
pub(super) fn after_volume<BackendError>(
    primary: &InitError<BackendError>,
    volume: &VolumeHandle,
    note: String,
    cleanup: Option<CleanupFailure>,
) -> InitError<BackendError>
where
    BackendError: std::error::Error + 'static,
{
    let kind = primary.kind();
    InitError::AfterVolume {
        primary: primary.to_string(),
        kind,
        volume_id: volume.id.clone(),
        zone: volume.zone.clone(),
        note,
        cleanup: cleanup.map(Box::new),
    }
}

/// Describes how to configure a retained volume, including the recovery step.
pub(super) fn retained_note(volume_id: &str) -> String {
    format!(
        "the new volume {volume_id} is retained; configure it with \
         [{SCALEWAY_SECTION}] {VOLUME_KEY} = \"{volume_id}\" or {VOLUME_ENV_VAR}={volume_id}, \
         then run `mriya recover` to confirm it"
    )
}

/// Describes a cleanup interrupted by a second signal.
pub(super) fn interrupted_note(ledger: &ResourceLedger) -> String {
    format!(
        "cleanup was interrupted by a second signal; run `mriya recover`{note}",
        note = journal_note(ledger.journal_path().as_deref())
    )
}

/// Renders the journal path suffix for a message.
pub(super) fn journal_note(journal: Option<&camino::Utf8Path>) -> String {
    journal.map_or_else(String::new, |path| format!(" (journal: {path})"))
}
