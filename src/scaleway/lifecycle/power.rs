//! Server power actions and the state waits that follow them.
//!
//! Cleanup powers a server off with the provider's own `poweroff` action and
//! waits for the `stopped` state before touching its volumes, so a running
//! guest cannot keep writing to a volume that is about to be detached.

use tokio::time::{Instant, sleep};

use super::{HTTP_CLIENT, InstanceSnapshot, http::checked_response};
use crate::scaleway::{ScalewayBackend, ScalewayBackendError, types::Zone};

impl ScalewayBackend {
    /// Issues a server action through the substitutable HTTP boundary.
    ///
    /// # Errors
    ///
    /// Returns a classified provider error.
    pub(in crate::scaleway) async fn server_action(
        &self,
        zone: &str,
        server_id: &str,
        action: &str,
    ) -> Result<(), ScalewayBackendError> {
        let url = self.instance_url(zone, &format!("servers/{server_id}/action"));
        checked_response(
            "server action",
            self.authorized(HTTP_CLIENT.post(&url))
                .json(&serde_json::json!({ "action": action })),
        )
        .await?;
        Ok(())
    }

    /// Powers on a stopped instance only when `poweron` is an allowed action.
    ///
    /// # Errors
    ///
    /// Returns `PowerOnNotAllowed` when the snapshot is not running and does
    /// not offer `poweron`, and propagates a provider error from the power-on
    /// request.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // A running snapshot needs no request at all.
    /// backend.power_on_if_needed(&zone, &running_snapshot).await?;
    ///
    /// // A stopped snapshot offering only `reboot` cannot be powered on.
    /// let err = backend.power_on_if_needed(&zone, &reboot_only).await.unwrap_err();
    /// assert!(matches!(err, ScalewayBackendError::PowerOnNotAllowed { .. }));
    /// ```
    pub(in crate::scaleway) async fn power_on_if_needed(
        &self,
        zone: &Zone,
        snapshot: &InstanceSnapshot,
    ) -> Result<(), ScalewayBackendError> {
        if snapshot.state.as_str() == "running" {
            return Ok(());
        }

        if snapshot
            .allowed_actions
            .iter()
            .any(|action| action.as_str() == "poweron")
        {
            self.server_action(zone.as_str(), snapshot.id.as_str(), "poweron")
                .await?;
            return Ok(());
        }

        Err(ScalewayBackendError::PowerOnNotAllowed {
            instance_id: snapshot.id.as_str().to_owned(),
            state: snapshot.state.as_str().to_owned(),
        })
    }

    /// Powers a server off when it is not already stopped, then waits.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the action is rejected, and
    /// [`ScalewayBackendError::Timeout`] when the server never reports
    /// `stopped`.
    ///
    /// `project_id` must be the project the server was recorded in: the wait
    /// confirms an eventual 404 by listing that project, so using the
    /// backend's default would confirm absence from the wrong project and let
    /// cleanup detach volumes from a server that is still running.
    pub(in crate::scaleway) async fn power_off_if_needed(
        &self,
        zone: &str,
        snapshot: &InstanceSnapshot,
        project_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        if snapshot.state.as_str() == "stopped" {
            return Ok(());
        }

        self.server_action(zone, snapshot.id.as_str(), "poweroff")
            .await?;
        self.wait_until_stopped(zone, snapshot.id.as_str(), project_id)
            .await
    }

    /// Polls until the server reports `stopped` or is confirmed absent.
    ///
    /// A 404 alone is not proof: as everywhere else, absence is confirmed by a
    /// successful, identity-checked listing. Without that check a transient
    /// 404 would let cleanup detach volumes from a guest that is still running.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Timeout`] when the deadline passes with
    /// the server still not stopped, and any other provider failure.
    pub(in crate::scaleway) async fn wait_until_stopped(
        &self,
        zone: &str,
        server_id: &str,
        project_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            let finished = match self.get_server(zone, server_id).await {
                Ok(server) => server.state == "stopped",
                // A listing that still finds the server keeps the loop going;
                // only confirmed absence ends it.
                Err(ScalewayBackendError::NotFound { .. }) => {
                    self.confirmed_absent(zone, server_id, project_id).await?
                }
                Err(error) => return Err(error),
            };
            if finished {
                return Ok(());
            }

            if Instant::now() > deadline {
                return Err(ScalewayBackendError::Timeout {
                    action: String::from("wait for the server to stop"),
                    instance_id: server_id.to_owned(),
                });
            }
            sleep(self.poll_interval).await;
        }
    }

    /// Confirms a server's absence with an identity-checked listing.
    async fn confirmed_absent(
        &self,
        zone: &str,
        server_id: &str,
        project_id: &str,
    ) -> Result<bool, ScalewayBackendError> {
        let resource = crate::ownership::ResourceRef::server(server_id, zone);
        Ok(self
            .server_presence(&resource, project_id)
            .await?
            .is_absent())
    }
}
