//! Failure-kind classification for init scenarios.

use mriya::InitError;

use super::test_helpers::InitFailureKind;

pub(super) fn map_failure_kind(
    err: &InitError<super::test_doubles::ScriptedVolumeBackendError>,
) -> InitFailureKind {
    match err {
        InitError::Config(_) => InitFailureKind::Config,
        InitError::Volume(_) | InitError::VolumeReady { .. } => InitFailureKind::Volume,
        InitError::Provision(_) => InitFailureKind::Provision,
        InitError::Wait { .. } => InitFailureKind::Wait,
        InitError::FormatDeviceNotFound { .. }
        | InitError::FormatDeviceAmbiguous { .. }
        | InitError::FormatRootDisk { .. }
        | InitError::FormatRootUnknown { .. }
        | InitError::FormatSignaturePresent { .. }
        | InitError::FormatFailed { .. } => InitFailureKind::Format,
        InitError::Detach { .. } => InitFailureKind::Detach,
        InitError::FormatterTeardown { .. } | InitError::TeardownAfterDetach { .. } => {
            InitFailureKind::Teardown
        }
        InitError::Rollback { original, .. } => original
            .downcast_ref::<InitError<super::test_doubles::ScriptedVolumeBackendError>>()
            .map_or(InitFailureKind::Format, map_failure_kind),
    }
}

pub(super) fn parse_failure_kind(kind: &str) -> Option<InitFailureKind> {
    match kind {
        "config" => Some(InitFailureKind::Config),
        "volume" => Some(InitFailureKind::Volume),
        "provision" => Some(InitFailureKind::Provision),
        "wait" => Some(InitFailureKind::Wait),
        "format" => Some(InitFailureKind::Format),
        "detach" => Some(InitFailureKind::Detach),
        "teardown" => Some(InitFailureKind::Teardown),
        _ => None,
    }
}
