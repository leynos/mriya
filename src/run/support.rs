//! Small helpers shared by the run steps.

use super::StepFailure;
use crate::{cancel::Cancellation, sync::RemoteCommandOutput};

/// Returns `Cancelled` when a signal has already arrived.
pub(super) fn check_cancelled<BackendError>(
    cancellation: &Cancellation,
) -> Result<(), StepFailure<BackendError>>
where
    BackendError: std::error::Error + 'static,
{
    if cancellation.is_cancelled() {
        return Err(StepFailure::Cancelled);
    }
    Ok(())
}

/// Describes a non-success remote status, including trimmed stderr when there
/// is any.
pub(super) fn remote_status_detail(output: &RemoteCommandOutput) -> String {
    let stderr = output.stderr.trim();
    match (output.exit_code, stderr.is_empty()) {
        (Some(code), true) => format!("remote command exited with status {code}"),
        (Some(code), false) => format!("remote command exited with status {code}: {stderr}"),
        (None, true) => String::from("remote command terminated without an exit status"),
        (None, false) => {
            format!("remote command terminated without an exit status: {stderr}")
        }
    }
}
