//! Reconciliation of ambiguous create requests by allocation tag.
//!
//! A create whose response never arrived may still have been applied. The tag
//! written to the request before it was sent is the only durable correlation,
//! so reconciliation lists by tag, verifies the tag and scope locally, and
//! treats two different matches as ambiguous rather than guessing.

use tokio::time::{Instant, sleep};

use super::models::{ServerModel, VolumeModel};
use crate::{
    ownership::{AllocationScope, AllocationTag},
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

impl ScalewayBackend {
    /// Finds a server carrying `tag` with a single listing.
    ///
    /// Cleanup uses this to discover an allocation whose create response was
    /// lost. One successful listing is authoritative at that point: the create
    /// either completed or it did not, and a journal whose outcome is still
    /// unknown is kept for `mriya recover` rather than polled here.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::AmbiguousOutcome`] when more than one
    /// server carries the tag, and any listing failure.
    pub(in crate::scaleway) async fn find_server_by_tag(
        &self,
        tag: &AllocationTag,
        scope: &AllocationScope,
    ) -> Result<Option<ServerModel>, ScalewayBackendError> {
        let matches = self
            .list_servers(&scope.zone, &scope.project_id, Some(tag.as_str()))
            .await?;
        match matches.as_slice() {
            [] => Ok(None),
            [server] => Ok(Some(server.clone())),
            several => Err(ScalewayBackendError::AmbiguousOutcome {
                operation: String::from("reconcile server"),
                message: format!(
                    "{} servers carry allocation tag {tag} in zone {}",
                    several.len(),
                    scope.zone
                ),
            }),
        }
    }

    /// Polls for a server carrying `tag` within the backend's wait window.
    ///
    /// Returns `Ok(None)` when the window closes with no match, which the
    /// caller reports as an unknown allocation outcome rather than an absence.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::AmbiguousOutcome`] when more than one
    /// server carries the tag, and any listing failure.
    pub(in crate::scaleway) async fn reconcile_server(
        &self,
        tag: &AllocationTag,
        scope: &AllocationScope,
    ) -> Result<Option<ServerModel>, ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            let matches = self
                .list_servers(&scope.zone, &scope.project_id, Some(tag.as_str()))
                .await?;
            match matches.as_slice() {
                [] => {}
                [server] => return Ok(Some(server.clone())),
                several => {
                    return Err(ScalewayBackendError::AmbiguousOutcome {
                        operation: String::from("reconcile server"),
                        message: format!(
                            "{} servers carry allocation tag {tag} in zone {}",
                            several.len(),
                            scope.zone
                        ),
                    });
                }
            }

            if Instant::now() > deadline {
                return Ok(None);
            }
            sleep(self.poll_interval).await;
        }
    }

    /// Polls for a volume carrying `tag` within the backend's wait window.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::AmbiguousOutcome`] when more than one
    /// volume carries the tag, and any listing failure.
    pub(in crate::scaleway) async fn reconcile_volume(
        &self,
        tag: &AllocationTag,
        scope: &AllocationScope,
    ) -> Result<Option<VolumeModel>, ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            let matches = self
                .list_instance_volumes(&scope.zone, &scope.project_id, Some(tag.as_str()))
                .await?;
            match matches.as_slice() {
                [] => {}
                [volume] => return Ok(Some(volume.clone())),
                several => {
                    return Err(ScalewayBackendError::AmbiguousOutcome {
                        operation: String::from("reconcile volume"),
                        message: format!(
                            "{} volumes carry allocation tag {tag} in zone {}",
                            several.len(),
                            scope.zone
                        ),
                    });
                }
            }

            if Instant::now() > deadline {
                return Ok(None);
            }
            sleep(self.poll_interval).await;
        }
    }
}
