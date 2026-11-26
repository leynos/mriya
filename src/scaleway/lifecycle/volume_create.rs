//! Block Storage volume creation helpers for the Scaleway backend.
//!
//! Volume creation uses the Instances API, which owns Basic Block Storage
//! (`b_ssd`) volumes. The allocation tag is written to the request before it is
//! sent, so an ambiguous create can be reconciled exactly as a server can.

use serde::{Deserialize, Serialize};

use super::{
    HTTP_CLIENT,
    http::{classify_error, request_error},
};
use crate::{
    ownership::{
        AllocationScope,
        AllocationTag,
        Disposition,
        ResourceKind,
        ResourceLedger,
        ResourceRef,
    },
    scaleway::{ScalewayBackend, ScalewayBackendError},
    volume::{VolumeHandle, VolumeRequest},
};

/// Volume type requested for cache volumes (Scaleway Block Storage SSD).
const VOLUME_TYPE_BLOCK: &str = "b_ssd";

/// JSON body for the create-volume call.
///
/// # Examples
///
/// ```ignore
/// // Untagged and organisation-less volumes omit those keys entirely.
/// let body = CreateVolumeRequest {
///     name: String::from("mriya-cache"),
///     size: 20_000_000_000,
///     volume_type: String::from(VOLUME_TYPE_BLOCK),
///     project: String::from("proj-1"),
///     organization: None,
///     tags: Vec::new(),
/// };
/// let json = serde_json::to_value(&body)?;
/// assert_eq!(json["volume_type"], "b_ssd");
/// assert!(json.get("organization").is_none());
/// assert!(json.get("tags").is_none());
/// ```
#[derive(Serialize)]
struct CreateVolumeRequest {
    /// Volume name as supplied by the caller.
    name: String,
    /// Volume size in bytes, taken from `VolumeRequest::size_bytes`.
    size: u64,
    /// Provider volume type; always `VOLUME_TYPE_BLOCK` here.
    volume_type: String,
    /// Project that will own the volume.
    project: String,
    /// Optional organization identifier, omitted from the body when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    organization: Option<String>,
    /// Tags for identifying cache volumes; omitted from the body when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

/// Wrapper for the volume object returned by a successful create call.
///
/// # Examples
///
/// ```ignore
/// // The provider nests the volume under `volume`. Both `id` and `zone` are
/// // required, so both must be present for this to deserialize.
/// let response: CreateVolumeResponse = serde_json::from_str(
///     r#"{"volume":{"id":"vol-1","zone":"fr-par-1"}}"#,
/// )?;
/// assert_eq!(response.volume.id, "vol-1");
/// assert_eq!(response.volume.zone, "fr-par-1");
/// ```
#[derive(Deserialize)]
struct CreateVolumeResponse {
    /// The created volume as reported by the provider.
    volume: ScalewayVolume,
}

/// The subset of the provider's volume object that creation reads.
///
/// # Examples
///
/// ```ignore
/// // Unknown provider fields are ignored; the identifier and zone are kept.
/// let volume: ScalewayVolume = serde_json::from_str(
///     r#"{"id":"vol-1","zone":"fr-par-1","extra":"ignored"}"#,
/// )?;
/// assert_eq!(volume.id, "vol-1");
/// assert_eq!(volume.zone, "fr-par-1");
/// ```
#[derive(Deserialize)]
struct ScalewayVolume {
    /// Provider identifier of the volume.
    id: String,
    /// Zone in which the provider placed the volume.
    #[serde(default)]
    zone: String,
}

impl ScalewayBackend {
    /// Creates a new Block Storage volume and records it as owned.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeCreateFailed`] when the provider
    /// rejects the creation request, a provider error when no response
    /// arrives, and [`ScalewayBackendError::AllocationOutcomeUnknown`] when an
    /// ambiguous request could not be reconciled by tag.
    pub(in crate::scaleway) async fn create_volume(
        &self,
        request: &VolumeRequest,
        ledger: &ResourceLedger,
    ) -> Result<VolumeHandle, ScalewayBackendError> {
        ledger
            .persist_initial()
            .map_err(|error| ScalewayBackendError::Journal {
                message: error.message,
            })?;

        let handle = match self.create_or_reconcile_volume(request, ledger).await {
            Ok(handle) => handle,
            Err(error @ ScalewayBackendError::AllocationOutcomeUnknown { .. }) => {
                ledger.mark_outcome_unknown();
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        ledger.record(
            ResourceRef::instance_volume(&handle.id, &handle.zone),
            Disposition::Owned,
        );
        Ok(handle)
    }

    /// Creates the volume, reconciling an ambiguous response by allocation tag.
    async fn create_or_reconcile_volume(
        &self,
        request: &VolumeRequest,
        ledger: &ResourceLedger,
    ) -> Result<VolumeHandle, ScalewayBackendError> {
        let tag = ledger.tag();
        let scope = ledger.scope();
        let url = self.instance_url(&request.zone, "volumes");
        let payload = CreateVolumeRequest {
            name: request.name.clone(),
            size: request.size_bytes,
            volume_type: String::from(VOLUME_TYPE_BLOCK),
            project: request.project_id.clone(),
            organization: request.organisation_id.clone(),
            tags: Self::volume_tags(self.test_run_id.as_deref(), Some(tag.as_str())),
        };

        let send = self
            .authorized(HTTP_CLIENT.post(&url))
            .json(&payload)
            .timeout(super::HTTP_TIMEOUT)
            .send()
            .await;

        let response = match send {
            Ok(response) => response,
            Err(error) if error.is_connect() => {
                return Err(request_error("create volume", &error));
            }
            Err(error) => {
                return self
                    .reconcile_created_volume(&tag, &scope, &error.to_string())
                    .await;
            }
        };

        let status = response.status();
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(error) => {
                return self
                    .reconcile_created_volume(&tag, &scope, &error.to_string())
                    .await;
            }
        };

        if status.is_success() {
            return match serde_json::from_slice::<CreateVolumeResponse>(&body) {
                Ok(parsed) => Ok(VolumeHandle::new(
                    parsed.volume.id,
                    if parsed.volume.zone.trim().is_empty() {
                        request.zone.clone()
                    } else {
                        parsed.volume.zone
                    },
                )),
                Err(error) => {
                    self.reconcile_created_volume(&tag, &scope, &error.to_string())
                        .await
                }
            };
        }

        let text = String::from_utf8_lossy(&body).into_owned();
        if status.is_client_error() {
            return Err(ScalewayBackendError::VolumeCreateFailed {
                name: request.name.clone(),
                zone: request.zone.clone(),
                message: classify_error("create volume", status, &text).to_string(),
            });
        }

        self.reconcile_created_volume(
            &tag,
            &scope,
            &format!("provider answered {}", status.as_u16()),
        )
        .await
    }

    /// Reconciles an ambiguous volume create, or reports an unknown outcome.
    async fn reconcile_created_volume(
        &self,
        tag: &AllocationTag,
        scope: &AllocationScope,
        detail: &str,
    ) -> Result<VolumeHandle, ScalewayBackendError> {
        self.reconcile_volume(tag, scope).await?.map_or_else(
            || {
                Err(ScalewayBackendError::AllocationOutcomeUnknown {
                    kind: ResourceKind::InstanceVolume,
                    tag: tag.to_string(),
                    zone: scope.zone.clone(),
                    detail: detail.to_owned(),
                })
            },
            |volume| {
                Ok(VolumeHandle::new(
                    volume.id,
                    if volume.zone.trim().is_empty() {
                        scope.zone.clone()
                    } else {
                        volume.zone
                    },
                ))
            },
        )
    }
}
