//! Recovery instructions for a failed ordered teardown.

use crate::{
    backend::InstanceHandle,
    volume::{TeardownStage, VolumeHandle},
};

/// Returns the recovery steps for the teardown stage that failed.
///
/// A stop or detach failure leaves the cache possibly attached, so the
/// volume must not be deleted before those steps succeed. A destroy
/// failure after detachment leaves only the formatter to remove.
pub(super) fn recovery_steps(
    stage: TeardownStage,
    handle: &InstanceHandle,
    volume: &VolumeHandle,
) -> String {
    match stage {
        TeardownStage::Stopping => format!(
            "stop server {} and detach volume {} in zone {} before deleting either",
            handle.id, volume.id, volume.zone
        ),
        TeardownStage::Detaching => format!(
            "detach volume {} from server {} with `scw instance server detach-volume server-id={} volume-id={} zone={}`",
            volume.id, handle.id, handle.id, volume.id, volume.zone
        ),
        TeardownStage::Destroying => format!(
            "the volume was detached; remove the formatter with `scw instance server delete {} zone={}`",
            handle.id, volume.zone
        ),
    }
}
