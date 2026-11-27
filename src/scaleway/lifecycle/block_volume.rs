//! Block Storage volume lookup, readiness polling and deletion.
//!
//! These helpers own the `/block/v1/zones/{zone}/volumes` contract. A lookup
//! that returns 404 is reported as [`ScalewayBackendError::VolumeNotInZone`]
//! because the caller supplies the zone and the usual cause is a volume in a
//! different Availability Zone. Deletion treats 404 as success so a retried
//! teardown is idempotent.

use std::{future::Future, time::Duration};

use tokio::time::{Instant, sleep};

use super::super::{ScalewayBackend, ScalewayBackendError, types::SbsVolume};
use crate::volume::{VolumeHandle, VolumeState, VolumeStatus};

impl ScalewayBackend {
    /// Reads one Block Storage volume from `zone`.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeNotInZone`] for a 404, a provider
    /// error for any other rejected response, and
    /// [`ScalewayBackendError::Provider`] when the success body cannot be
    /// parsed.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let state = backend.get_volume("fr-par-1", "11111111-1111-1111-1111-111111111111").await?;
    /// assert_eq!(state.status, VolumeStatus::Available);
    /// ```
    pub(in crate::scaleway) async fn get_volume(
        &self,
        zone: &str,
        volume_id: &str,
    ) -> Result<VolumeState, ScalewayBackendError> {
        let url = format!("{}/zones/{zone}/volumes/{volume_id}", self.block_api_base);
        let response = super::http::send_raw(
            super::HTTP_CLIENT
                .get(&url)
                .header("X-Auth-Token", &self.config.secret_key),
        )
        .await?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(ScalewayBackendError::VolumeNotInZone {
                volume_id: volume_id.to_owned(),
                zone: zone.to_owned(),
            });
        }
        if status.is_client_error() {
            return Err(super::http::api_error(response).await);
        }
        let checked = response
            .error_for_status()
            .map_err(|err| super::http::request_error(&err))?;
        let volume: SbsVolume =
            checked
                .json()
                .await
                .map_err(|err| ScalewayBackendError::Provider {
                    message: err.to_string(),
                })?;
        Ok(volume.into_state())
    }

    /// Polls until the volume reports `expected`, returning its final state.
    ///
    /// The backend's polling interval and wait timeout apply, matching the
    /// instance readiness waits. An `error` or `deleted` status fails
    /// immediately rather than waiting for the deadline.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeUnavailable`] for a terminal
    /// provider failure, [`ScalewayBackendError::VolumeStatusTimeout`] when the
    /// deadline elapses, and any lookup error from
    /// [`ScalewayBackend::get_volume`].
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Creation reports `creating` before the volume accepts an attachment.
    /// let state = backend
    ///     .wait_for_volume_status("fr-par-1", &handle.id, VolumeStatus::Available)
    ///     .await?;
    /// assert_eq!(state.status, VolumeStatus::Available);
    /// ```
    pub(in crate::scaleway) async fn wait_for_volume_status(
        &self,
        zone: &str,
        volume_id: &str,
        expected: VolumeStatus,
    ) -> Result<VolumeState, ScalewayBackendError> {
        poll_for_volume_status(
            volume_id,
            &expected,
            VolumePollSettings {
                poll_interval: self.poll_interval,
                wait_timeout: self.wait_timeout,
            },
            || self.get_volume(zone, volume_id),
        )
        .await
    }

    /// Deletes a volume that the current operation owns.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeDeleteFailed`] for a rejected
    /// deletion. A 404 is treated as success because the volume is already
    /// gone, which makes a retried teardown idempotent.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // A successful or already-deleted volume both resolve to `Ok`.
    /// backend.delete_volume(&handle).await?;
    /// ```
    pub(in crate::scaleway) async fn delete_volume(
        &self,
        handle: &VolumeHandle,
    ) -> Result<(), ScalewayBackendError> {
        let url = format!(
            "{}/zones/{}/volumes/{}",
            self.block_api_base, handle.zone, handle.id
        );
        let response = super::http::send_raw(
            super::HTTP_CLIENT
                .delete(&url)
                .header("X-Auth-Token", &self.config.secret_key),
        )
        .await?;

        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }

        let message = response.text().await.unwrap_or_default();
        Err(ScalewayBackendError::VolumeDeleteFailed {
            volume_id: handle.id.clone(),
            message,
        })
    }
}

/// Timing controls for the volume status poll.
#[derive(Clone, Copy, Debug)]
pub(super) struct VolumePollSettings {
    /// Delay between provider reads.
    pub(super) poll_interval: Duration,
    /// Total time allowed before a timeout.
    pub(super) wait_timeout: Duration,
}

/// Polls `fetch` until the volume reports `expected` or a terminal state.
///
/// Keeping the loop separate from the HTTP call lets unit tests drive the
/// production algorithm with scripted states, including the timeout path,
/// without scripting an unbounded number of responses.
///
/// # Errors
///
/// Returns [`ScalewayBackendError::VolumeUnavailable`] for `error` or
/// `deleted`, [`ScalewayBackendError::VolumeStatusTimeout`] when the deadline
/// elapses, and any error returned by `fetch`. A read that starts before the
/// deadline may finish after it and still be accepted when it reports the
/// expected status; the budget bounds the waiting, not the last in-flight
/// read.
pub(super) async fn poll_for_volume_status<F, Fut>(
    volume_id: &str,
    expected: &VolumeStatus,
    settings: VolumePollSettings,
    mut fetch: F,
) -> Result<VolumeState, ScalewayBackendError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<VolumeState, ScalewayBackendError>>,
{
    let deadline = Instant::now() + settings.wait_timeout;
    let mut last_status;

    loop {
        let state = fetch().await?;
        if state.status == *expected {
            return Ok(state);
        }
        if matches!(state.status, VolumeStatus::Error | VolumeStatus::Deleted) {
            return Err(ScalewayBackendError::VolumeUnavailable {
                volume_id: volume_id.to_owned(),
                status: state.status.to_string(),
            });
        }

        last_status = state.status.to_string();
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        sleep(settings.poll_interval.min(remaining)).await;
    }

    Err(ScalewayBackendError::VolumeStatusTimeout {
        volume_id: volume_id.to_owned(),
        expected: expected.to_string(),
        last_status,
    })
}
