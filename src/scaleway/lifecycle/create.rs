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

#[derive(Serialize)]
struct CreateServerRequest {
    name: String,
    commercial_type: String,
    image: String,
    project: String,
    routed_ip_enabled: bool,
    dynamic_ip_required: bool,
    tags: Vec<String>,
    stopped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_init: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    organization: Option<String>,
}

#[derive(Deserialize)]
struct CreateServerResponse {
    server: scaleway_rs::ScalewayInstance,
}

impl ScalewayBackend {
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
            self.api
                .perform_instance_action_async(zone.as_str(), snapshot.id.as_str(), "poweron")
                .await?;
            return Ok(());
        }

        Err(ScalewayBackendError::PowerOnNotAllowed {
            instance_id: snapshot.id.as_str().to_owned(),
            state: snapshot.state.as_str().to_owned(),
        })
    }

    /// Creates a Scaleway instance in a stopped state.
    ///
    /// The instance is created with `stopped: true` so that optional cloud-init
    /// user-data can be supplied in the creation request and consumed on the
    /// first boot after the instance is powered on.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError`] when the Scaleway API request fails or
    /// the provider rejects the requested instance type or image.
    pub(in crate::scaleway) async fn create_instance_stopped(
        &self,
        request: &InstanceRequest,
        image_id: &str,
    ) -> Result<scaleway_rs::ScalewayInstance, ScalewayBackendError> {
        let url = format!(
            "{}/zones/{}/servers",
            super::SCALEWAY_INSTANCE_API_BASE,
            request.zone
        );
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
