//! Server creation with allocation tagging and ambiguous-outcome recovery.
//!
//! Scaleway instances must receive cloud-init user-data before first boot, so
//! the request sets `stopped: true` and the caller powers the server on once
//! the cache volume is attached. The allocation tag is written to the request
//! before it is sent, which is what lets an interrupted or ambiguous create be
//! reconciled from the provider's own metadata.

use scaleway_rs::ScalewayApiError;
use serde::Serialize;

use super::{
    HTTP_CLIENT,
    http::{classify_error, request_error},
    models::{ServerModel, ServerResponse},
};
use crate::{
    backend::InstanceRequest,
    ownership::{AllocationScope, AllocationTag, ResourceKind, ResourceLedger},
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

/// JSON body for the create-server call.
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
    /// Generated instance name, `mriya-` followed by the allocation suffix.
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

/// A create-server response awaiting classification.
struct CreateResponse<'a> {
    /// The request that produced the response.
    request: &'a InstanceRequest,
    /// Allocation tag used to reconcile an ambiguous outcome.
    tag: &'a AllocationTag,
    /// Scope used to reconcile an ambiguous outcome.
    scope: &'a AllocationScope,
    /// HTTP status the provider returned.
    status: reqwest::StatusCode,
    /// Response body bytes.
    body: &'a [u8],
}

impl ScalewayBackend {
    /// Creates a Scaleway instance in a stopped state.
    ///
    /// The body names the server after the allocation tag so an operator
    /// reading the console sees the same identifier the journal records.
    ///
    /// # Errors
    ///
    /// Returns `InstanceTypeUnavailable` when the provider identifies the
    /// requested type as the problem, a classified provider error for other
    /// rejections, and [`ScalewayBackendError::AllocationOutcomeUnknown`] when
    /// an ambiguous request could not be reconciled by tag.
    pub(in crate::scaleway) async fn create_instance_stopped(
        &self,
        request: &InstanceRequest,
        image_id: &str,
        ledger: &ResourceLedger,
    ) -> Result<ServerModel, ScalewayBackendError> {
        let tag = ledger.tag();
        let scope = ledger.scope();
        let url = self.instance_url(&request.zone, "servers");
        let payload = CreateServerRequest {
            name: format!("mriya-{}", tag.suffix()),
            commercial_type: request.instance_type.clone(),
            image: image_id.to_owned(),
            project: request.project_id.clone(),
            routed_ip_enabled: true,
            dynamic_ip_required: true,
            tags: Self::instance_tags(self.test_run_id.as_deref(), Some(tag.as_str())),
            stopped: true,
            cloud_init: request.cloud_init_user_data.clone(),
            organization: request.organisation_id.clone(),
        };

        let send = self
            .authorized(HTTP_CLIENT.post(&url))
            .json(&payload)
            .timeout(super::HTTP_TIMEOUT)
            .send()
            .await;

        let response = match send {
            Ok(response) => response,
            // A connection failure means the request never reached the
            // provider, so nothing was created and reconciliation would only
            // waste the wait window.
            Err(error) if error.is_connect() => {
                return Err(request_error("create server", &error));
            }
            Err(error) => {
                return self
                    .reconcile_or_unknown(&tag, &scope, &error.to_string())
                    .await;
            }
        };

        let status = response.status();
        let body = match response.bytes().await {
            Ok(body) => body,
            // A definitive client rejection stays a rejection even when its
            // body is unreadable: the provider decided, so the request was not
            // applied and reconciliation would only waste the wait window.
            Err(error) if status.is_client_error() => {
                return Err(classify_error(
                    "create server",
                    status,
                    &format!("response body could not be read: {error}"),
                ));
            }
            Err(error) => {
                return self
                    .reconcile_or_unknown(&tag, &scope, &error.to_string())
                    .await;
            }
        };

        self.classify_create_response(CreateResponse {
            request,
            tag: &tag,
            scope: &scope,
            status,
            body: &body,
        })
        .await
    }

    /// Maps a create response body to a server, a rejection, or a
    /// reconciliation attempt.
    async fn classify_create_response(
        &self,
        response: CreateResponse<'_>,
    ) -> Result<ServerModel, ScalewayBackendError> {
        let CreateResponse {
            request,
            tag,
            scope,
            status,
            body,
        } = response;
        if status.is_success() {
            return match serde_json::from_slice::<ServerResponse>(body) {
                Ok(parsed) => Ok(parsed.server),
                Err(error) => {
                    self.reconcile_or_unknown(tag, scope, &error.to_string())
                        .await
                }
            };
        }

        let text = String::from_utf8_lossy(body).into_owned();
        if status.is_client_error() {
            if let Ok(api_error) = serde_json::from_slice::<ScalewayApiError>(body)
                && Self::is_instance_type_error(&api_error, request)
            {
                return Err(ScalewayBackendError::InstanceTypeUnavailable {
                    instance_type: request.instance_type.clone(),
                    zone: request.zone.clone(),
                });
            }
            return Err(classify_error("create server", status, &text));
        }

        self.reconcile_or_unknown(
            tag,
            scope,
            &format!("provider answered {}", status.as_u16()),
        )
        .await
    }

    /// Reconciles an ambiguous create, or reports that the outcome is unknown.
    async fn reconcile_or_unknown(
        &self,
        tag: &AllocationTag,
        scope: &AllocationScope,
        detail: &str,
    ) -> Result<ServerModel, ScalewayBackendError> {
        let unknown = |reason: String| ScalewayBackendError::AllocationOutcomeUnknown {
            kind: ResourceKind::Server,
            tag: tag.to_string(),
            zone: scope.zone.clone(),
            detail: reason,
        };
        // A failed lookup is exactly the unknown case: the create may have been
        // applied, and the tag is the only way to find it later.
        match self.reconcile_server(tag, scope).await {
            Ok(Some(server)) => Ok(server),
            Ok(None) => Err(unknown(detail.to_owned())),
            Err(error) => Err(unknown(format!("{detail}; reconciliation failed: {error}"))),
        }
    }
}
