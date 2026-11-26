//! Volume-map updates shared by attachment and detachment.
//!
//! The Instances API replaces the entire volume map on `PATCH /servers/{id}`,
//! so the caller supplies every volume to keep. Each update is reported with
//! the direction it took, which is what lets a rejection name the volume and
//! whether mriya was attaching or detaching it.

use crate::{
    backend::InstanceHandle,
    scaleway::{ScalewayBackend, ScalewayBackendError, volume::UpdateInstanceVolumesRequest},
};

/// Direction of a volume PATCH, used to choose the matching error variant.
///
/// The action-specific variants describe only a *non-success HTTP response*.
/// A request that never produced a response, such as a connection failure or
/// an elapsed timeout, yields `ScalewayBackendError::Provider` instead, because
/// there is no status for the action to describe.
#[derive(Copy, Clone, Debug)]
pub(super) enum VolumePatchAction {
    /// The PATCH adds a volume; a rejected response maps to
    /// `VolumeAttachmentFailed`.
    Attach,
    /// The PATCH removes a volume; a rejected response maps to
    /// `VolumeDetachFailed`.
    Detach,
}

impl VolumePatchAction {
    /// Builds the error variant matching this action from the failure details.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let err = VolumePatchAction::Attach.into_error(
    ///     String::from("vol-1"),
    ///     String::from("srv-1"),
    ///     String::from("conflict"),
    /// );
    /// assert!(matches!(err, ScalewayBackendError::VolumeAttachmentFailed { .. }));
    /// ```
    const fn into_error(
        self,
        volume_id: String,
        instance_id: String,
        message: String,
    ) -> ScalewayBackendError {
        match self {
            Self::Attach => ScalewayBackendError::VolumeAttachmentFailed {
                volume_id,
                instance_id,
                message,
            },
            Self::Detach => ScalewayBackendError::VolumeDetachFailed {
                volume_id,
                instance_id,
                message,
            },
        }
    }
}

/// Identifies the volume and direction of a PATCH.
///
/// # Examples
///
/// ```ignore
/// let attach = VolumePatchContext::attach("vol-1");
/// assert_eq!(attach.action, VolumePatchAction::Attach);
/// let detach = VolumePatchContext::detach("vol-1");
/// assert_eq!(detach.action, VolumePatchAction::Detach);
/// ```
#[derive(Copy, Clone, Debug)]
pub(super) struct VolumePatchContext<'a> {
    /// Identifier of the volume being attached or detached.
    volume_id: &'a str,
    /// Whether the PATCH attaches or detaches the volume.
    action: VolumePatchAction,
}

impl<'a> VolumePatchContext<'a> {
    /// Context for attaching `volume_id`.
    pub(super) const fn attach(volume_id: &'a str) -> Self {
        Self {
            volume_id,
            action: VolumePatchAction::Attach,
        }
    }

    /// Context for detaching `volume_id`.
    pub(super) const fn detach(volume_id: &'a str) -> Self {
        Self {
            volume_id,
            action: VolumePatchAction::Detach,
        }
    }
}

/// Sends a volume-map PATCH to the instance.
///
/// # Errors
///
/// A request that fails to send returns [`ScalewayBackendError::Provider`].
/// Only a response with a non-success status reaches
/// [`VolumePatchAction::into_error`].
pub(super) async fn patch_instance_volumes(
    backend: &ScalewayBackend,
    handle: &InstanceHandle,
    request: &UpdateInstanceVolumesRequest,
    context: VolumePatchContext<'_>,
) -> Result<(), ScalewayBackendError> {
    let url = backend.instance_url(&handle.zone, &format!("servers/{}", handle.id));

    let response = backend
        .authorized(super::HTTP_CLIENT.patch(&url))
        .json(request)
        .timeout(super::HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|err| ScalewayBackendError::Provider {
            message: err.to_string(),
        })?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }

    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("response body could not be read: {error}"));
    // A 404 while detaching means the server is already gone; that is absence,
    // not a detached-failure, and the caller treats it as such.
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(ScalewayBackendError::NotFound {
            operation: String::from("patch instance volumes"),
            message: body,
        });
    }
    Err(context.action.into_error(
        context.volume_id.to_owned(),
        handle.id.clone(),
        format!("HTTP {}: {body}", status.as_u16()),
    ))
}
