//! Instance creation helpers for the Scaleway backend.
//!
//! Scaleway instances must receive cloud-init user-data before first boot.
//! The creation request sets `stopped: true` so the payload is available when
//! the instance is powered on.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::InstanceRequest;
use crate::scaleway::types::Zone;

use super::super::{ScalewayBackend, ScalewayBackendError};
use super::InstanceSnapshot;

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
/// // The provider nests the instance under `server`.
/// let response: CreateServerResponse =
///     serde_json::from_str(r#"{"server":{"id":"srv-1","state":"stopped"}}"#)?;
/// assert_eq!(response.server.id, "srv-1");
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
            name,
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

        let response = super::HTTP_CLIENT
            .post(&url)
            .header("X-Auth-Token", &self.config.secret_key)
            .json(&payload)
            .send()
            .await
            .map_err(|err| ScalewayBackendError::Provider {
                message: err.to_string(),
            })?;

        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|err| ScalewayBackendError::Provider {
                message: err.to_string(),
            })?;

        if status.is_success() {
            let parsed: CreateServerResponse =
                serde_json::from_slice(&body).map_err(|err| ScalewayBackendError::Provider {
                    message: err.to_string(),
                })?;
            return Ok(parsed.server);
        }

        let message = String::from_utf8_lossy(&body).into_owned();
        if let Ok(api_err) = serde_json::from_slice::<scaleway_rs::ScalewayApiError>(&body)
            && Self::is_instance_type_error(&api_err, request)
        {
            return Err(ScalewayBackendError::InstanceTypeUnavailable {
                instance_type: request.instance_type.clone(),
                zone: request.zone.clone(),
            });
        }

        Err(ScalewayBackendError::Provider { message })
    }
}
