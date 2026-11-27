//! Instance creation helpers for the Scaleway backend.
//!
//! Scaleway instances must receive cloud-init user-data before first boot.
//! The creation request sets `stopped: true` so the payload is available when
//! the instance is powered on.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use uuid::Uuid;

use super::{
    super::{ScalewayBackend, ScalewayBackendError},
    InstanceSnapshot,
};
use crate::{
    backend::{Backend, InstanceHandle, InstanceRequest},
    scaleway::types::Zone,
};

/// JSON body for the create-server call.
///
/// This mirrors what the code currently sends, including `stopped` and
/// `cloud_init`; see issue #176 for the provider's response to those keys.
///
/// # Examples
///
/// ```ignore
/// // An unset cloud-init and organization are omitted from the body.
/// let body = CreateServerRequest {
///     name: String::from("mriya-abc"),
///     commercial_type: String::from("DEV1-S"),
///     image: String::from("img-1"),
///     project: String::from("proj-1"),
///     routed_ip_enabled: true,
///     dynamic_ip_required: true,
///     tags: vec![String::from("mriya")],
///     stopped: true,
///     cloud_init: None,
///     organization: None,
/// };
/// let json = serde_json::to_value(&body)?;
/// assert!(json.get("cloud_init").is_none());
/// ```
#[derive(Serialize)]
struct CreateServerRequest {
    /// Generated instance name, `mriya-` followed by a random UUID.
    name: String,
    /// Instance type to provision, such as `DEV1-S`.
    commercial_type: String,
    /// Identifier of the image to boot from.
    image: String,
    /// Project that will own the instance.
    project: String,
    /// Requests a routed IP address for the instance.
    routed_ip_enabled: bool,
    /// Requests that a dynamic public IP address be allocated.
    dynamic_ip_required: bool,
    /// Tags for identifying and cleaning up mriya-created instances.
    tags: Vec<String>,
    /// Requests that the instance be created without being started.
    stopped: bool,
    /// Optional cloud-init user-data, omitted from the body when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_init: Option<String>,
    /// Optional organization identifier, omitted from the body when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    organization: Option<String>,
}

/// Wrapper for the server object returned by a successful create call.
///
/// # Examples
///
/// ```ignore
/// // Schematic: the provider nests the instance under `server`. A real
/// // `ScalewayInstance` carries about thirty required fields with no serde
/// // defaults, so abbreviated JSON would fail to deserialize before any
/// // assertion ran. The decodable fixture used by the tests is
/// // `src/scaleway/lifecycle/tests/fixtures/detach-instance.json`.
/// {"server": {"id": "srv-1", "state": "stopped", ...}}
/// ```
#[derive(Deserialize)]
struct CreateServerResponse {
    /// The created instance as reported by the provider.
    server: scaleway_rs::ScalewayInstance,
}

impl ScalewayBackend {
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
            self.power_on_instance(zone, snapshot.id.as_str()).await?;
            return Ok(());
        }

        Err(ScalewayBackendError::PowerOnNotAllowed {
            instance_id: snapshot.id.as_str().to_owned(),
            state: snapshot.state.as_str().to_owned(),
        })
    }

    /// Powers the instance off unless it is already stopped, then waits for
    /// the `stopped` state.
    ///
    /// Scaleway only permits volume detachment while the server is stopped, so
    /// teardown calls this before detaching the cache.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the power-off action is rejected, and
    /// [`ScalewayBackendError::Timeout`] when the server never reports
    /// `stopped`. A server that has already disappeared counts as stopped.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // The companion instance is running after formatting; this stops it
    /// // before the cache is detached.
    /// backend.power_off_if_needed(&handle).await?;
    /// ```
    pub(in crate::scaleway) async fn power_off_if_needed(
        &self,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        let Some(snapshot) = self.fetch_instance(handle).await? else {
            return Ok(());
        };

        match snapshot.state.as_str() {
            "stopped" => return Ok(()),
            "stopping" => return self.wait_for_stopped(handle).await,
            _ => {
                let zone = Zone::from(handle.zone.as_str());
                self.power_off_instance(&zone, &handle.id).await?;
            }
        }

        self.wait_for_stopped(handle).await
    }

    /// Polls until the instance reports `stopped` or disappears.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Timeout`] when the deadline elapses
    /// before the server reports `stopped`.
    pub(in crate::scaleway) async fn wait_for_stopped(
        &self,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            match self.fetch_instance(handle).await? {
                None => return Ok(()),
                Some(snapshot) if snapshot.state.as_str() == "stopped" => return Ok(()),
                Some(_) => {}
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            sleep(self.poll_interval.min(remaining)).await;
            if Instant::now() >= deadline {
                break;
            }
        }

        Err(ScalewayBackendError::Timeout {
            action: String::from("wait_for_stopped"),
            instance_id: handle.id.clone(),
        })
    }

    /// Creates a Scaleway instance in a stopped state so optional cloud-init
    /// user-data can be supplied before first boot.
    ///
    /// # Errors
    ///
    /// Returns `Provider`/network failures, and `InstanceTypeUnavailable` when
    /// the requested instance type is not available in the selected zone.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // A well-formed request returns the instance as the provider reports it.
    /// let instance = backend.create_instance_stopped(&request, "img-1").await?;
    /// assert_eq!(instance.state, "stopped");
    ///
    /// // An instance type the zone does not offer is reported by name.
    /// let err = backend.create_instance_stopped(&request, "img-1").await.unwrap_err();
    /// assert!(matches!(err, ScalewayBackendError::InstanceTypeUnavailable { .. }));
    /// ```
    pub(in crate::scaleway) async fn create_instance_stopped(
        &self,
        request: &InstanceRequest,
        image_id: &str,
    ) -> Result<scaleway_rs::ScalewayInstance, ScalewayBackendError> {
        let url = format!("{}/zones/{}/servers", self.instance_api_base, request.zone);
        let name = format!("mriya-{}", Uuid::new_v4().simple());
        let tags = Self::instance_tags(self.test_run_id.as_deref());
        let payload = CreateServerRequest {
            name: name.clone(),
            commercial_type: request.instance_type.clone(),
            image: image_id.to_owned(),
            project: request.project_id.clone(),
            routed_ip_enabled: true,
            dynamic_ip_required: true,
            tags,
            stopped: true,
            cloud_init: request.cloud_init_user_data.clone(),
            organization: request.organisation_id.clone(),
        };

        // A transport error, a failed body read or an unparsable success body
        // all leave the outcome unknown: the server may exist. Reconcile it
        // before reporting, so a retry does not leak a server.
        let response = match super::http::send_raw(
            super::HTTP_CLIENT
                .post(&url)
                .header("X-Auth-Token", &self.config.secret_key)
                .json(&payload),
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                return self
                    .fail_ambiguous_create(request, &name, err.to_string())
                    .await;
            }
        };

        let status = response.status();
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(err) => {
                return self
                    .fail_ambiguous_create(request, &name, err.to_string())
                    .await;
            }
        };

        if status.is_success() {
            let parsed: CreateServerResponse = match serde_json::from_slice(&body) {
                Ok(parsed) => parsed,
                Err(err) => {
                    return self
                        .fail_ambiguous_create(request, &name, err.to_string())
                        .await;
                }
            };
            return Ok(parsed.server);
        }

        self.create_failure(request, &name, (&status, &body)).await
    }

    /// Maps a non-success create response to its typed error.
    async fn create_failure(
        &self,
        request: &InstanceRequest,
        name: &str,
        response: (&reqwest::StatusCode, &[u8]),
    ) -> Result<scaleway_rs::ScalewayInstance, ScalewayBackendError> {
        let (status, body) = response;
        let message = String::from_utf8_lossy(body).into_owned();
        // A 5xx can follow a server that was created before the gateway
        // failed, so it is ambiguous and must be reconciled before any
        // classification of its body; a 4xx is a definite rejection.
        if status.is_server_error() {
            return self
                .fail_ambiguous_create(
                    request,
                    name,
                    format!("create returned {status}: {message}"),
                )
                .await;
        }
        if let Ok(api_err) = serde_json::from_slice::<scaleway_rs::ScalewayApiError>(body)
            && Self::is_instance_type_error(&api_err, request)
        {
            return Err(ScalewayBackendError::InstanceTypeUnavailable {
                instance_type: request.instance_type.clone(),
                zone: request.zone.clone(),
            });
        }

        Err(ScalewayBackendError::Provider { message })
    }

    /// Reconciles a create whose outcome is unknown, then reports the failure.
    ///
    /// Every server carrying the generated name in the requested project and
    /// zone is removed before the error is returned. The name is unique to
    /// this call, so nothing created by another run can match.
    async fn fail_ambiguous_create(
        &self,
        request: &InstanceRequest,
        name: &str,
        message: String,
    ) -> Result<scaleway_rs::ScalewayInstance, ScalewayBackendError> {
        let error = ScalewayBackendError::Provider { message };
        match self
            .remove_servers_named(&request.zone, &request.project_id, name)
            .await
        {
            Ok(()) => Err(error),
            Err(cleanup) => Err(ScalewayBackendError::RollbackFailed {
                original: Box::new(error),
                cleanup: format!("the create outcome was unknown and cleanup failed: {cleanup}"),
            }),
        }
    }

    /// Deletes every server carrying `name` in `zone` and `project_id`.
    async fn remove_servers_named(
        &self,
        zone: &str,
        project_id: &str,
        name: &str,
    ) -> Result<(), ScalewayBackendError> {
        for id in self.find_instances_named(zone, project_id, name).await? {
            let handle = InstanceHandle {
                id,
                zone: zone.to_owned(),
            };
            Backend::destroy(self, handle).await?;
        }
        Ok(())
    }
}
