//! Instance creation helpers for the Scaleway backend.
//!
//! Scaleway instances must receive cloud-init user-data before first boot.
//! The creation request sets `stopped: true` so the payload is available when
//! the instance is powered on, and names the boot volume explicitly so the
//! disk that gets provisioned is the one the plan chose rather than a
//! provider default.
//!
//! The response is decoded with a local minimal model rather than the SDK's
//! `ScalewayInstance`. The SDK type drops the `boot` flag on volume entries,
//! which is exactly the field that identifies the root volume, so it cannot
//! answer the question this module asks.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    super::{ScalewayBackend, ScalewayBackendError},
    InstanceSnapshot,
};
use crate::{
    backend::InstanceRequest,
    boot_volume::{BootVolumePlan, BootVolumeType},
    scaleway::types::{Action, Zone},
};

/// Suffix `boot` carries on the single volume entry the create request sends.
const BOOT_VOLUME_INDEX: &str = "0";

/// JSON body for the create-server call.
///
/// This mirrors what the code sends, including `stopped`, `cloud_init`, and
/// the explicit boot volume; see issue #176 for the provider's response to
/// `stopped` and `cloud_init`.
///
/// # Examples
///
/// ```ignore
/// // An unset cloud-init and organization are omitted from the body, while the
/// // boot volume is always present at index "0".
/// let body = CreateServerRequest { .. };
/// let json = serde_json::to_value(&body)?;
/// assert!(json.get("cloud_init").is_none());
/// assert_eq!(json["volumes"]["0"]["boot"], true);
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
    /// The boot volume, always at index `"0"`.
    volumes: HashMap<String, CreateBootVolume>,
}

/// The `volumes["0"]` template for the boot disk.
///
/// `name` and `base_snapshot` are omitted: the provider derives the snapshot
/// from the `image` field, and the documented "create a volume from a snapshot
/// of an image" case leaves both optional.
///
/// # Examples
///
/// ```ignore
/// let template = CreateBootVolume {
///     volume_type: String::from("l_ssd"),
///     size: 20_000_000_000,
///     boot: true,
/// };
/// let json = serde_json::to_value(&template)?;
/// assert_eq!(json, serde_json::json!({
///     "volume_type": "l_ssd", "size": 20_000_000_000u64, "boot": true,
/// }));
/// ```
#[derive(Serialize)]
struct CreateBootVolume {
    /// Provider volume type, such as `l_ssd` or `sbs_volume`.
    volume_type: String,
    /// Disk size in bytes, aligned to the provider's 512-byte increment.
    size: u64,
    /// Marks this entry as the volume the instance boots from.
    boot: bool,
}

impl CreateBootVolume {
    /// Builds the boot entry from a resolved plan.
    ///
    /// For example, a 20 GB local plan yields `l_ssd`, `20_000_000_000`, and
    /// `boot: true`.
    fn from_plan(plan: &BootVolumePlan) -> Self {
        Self {
            volume_type: plan.volume_type.provider_id().to_owned(),
            size: plan.size_bytes,
            boot: true,
        }
    }

    /// The single index the boot entry occupies.
    const fn index() -> &'static str { BOOT_VOLUME_INDEX }
}

/// Wrapper for the server object returned by a successful create call.
///
/// Public within the crate so the decoder in `boot_capture` can name it.
///
/// # Examples
///
/// ```ignore
/// // Only the fields lifecycle creation reads are required; everything else
/// // the provider sends is ignored.
/// let response: CreateServerResponse = serde_json::from_str(
///     r#"{"server":{"id":"srv-1","state":"stopped"}}"#,
/// )?;
/// assert_eq!(response.server.id, "srv-1");
/// ```
#[derive(Deserialize)]
pub(in crate::scaleway) struct CreateServerResponse {
    /// The created instance as reported by the provider.
    pub(in crate::scaleway) server: CreatedServer,
}

/// The subset of the created server that creation and root capture read.
#[derive(Debug, Deserialize)]
pub(in crate::scaleway) struct CreatedServer {
    /// Provider identifier of the new instance.
    pub(in crate::scaleway) id: String,
    /// Provider-reported state, normally `stopped`.
    pub(in crate::scaleway) state: String,
    /// Actions the provider currently permits.
    #[serde(default)]
    pub(in crate::scaleway) allowed_actions: Vec<String>,
    /// Public IPv4 address, if one was allocated.
    #[serde(default)]
    pub(in crate::scaleway) public_ip: Option<CreatedPublicIp>,
    /// Attached volumes keyed by index, absent when the provider sent no
    /// `volumes` object.
    ///
    /// The distinction matters: an absent object is not evidence that nothing
    /// is attached, and a caller that treated it as such could conclude a cache
    /// volume was detached when it was not.
    #[serde(default)]
    pub(in crate::scaleway) volumes: Option<HashMap<String, CreatedVolume>>,
}

/// The public address object the provider nests under `public_ip`.
#[derive(Debug, Deserialize)]
pub(in crate::scaleway) struct CreatedPublicIp {
    /// Dotted-quad address.
    pub(in crate::scaleway) address: String,
}

/// One volume entry on a created server.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(in crate::scaleway) struct CreatedVolume {
    /// Provider identifier of the volume.
    pub(in crate::scaleway) id: String,
    /// Capacity in bytes, when the provider reports it.
    #[serde(default)]
    pub(in crate::scaleway) size: Option<u64>,
    /// Provider volume type, when the provider reports it.
    #[serde(default)]
    pub(in crate::scaleway) volume_type: Option<String>,
    /// True for the entry the instance boots from.
    #[serde(default)]
    pub(in crate::scaleway) boot: bool,
}

impl CreatedServer {
    /// Finds the volume the instance boots from.
    ///
    /// Prefers the entry the provider marks `boot`, and falls back to index
    /// `"0"` when no entry carries the flag. That fallback matters because the
    /// documented create response does not promise the flag on every provider
    /// version.
    ///
    /// For example, a server whose only volume is at `"0"` with `boot: false`
    /// still yields that volume.
    pub(in crate::scaleway) fn boot_volume(&self) -> Option<(&str, &CreatedVolume)> {
        let volumes = self.volumes.as_ref()?;
        volumes
            .iter()
            .find(|(_, volume)| volume.boot)
            .map(|(index, volume)| (index.as_str(), volume))
            .or_else(|| {
                volumes
                    .get_key_value(CreateBootVolume::index())
                    .map(|(index, volume)| (index.as_str(), volume))
            })
    }

    /// Builds the snapshot `power_on_if_needed` consumes.
    ///
    /// Borrows rather than consumes because the volumes map is still needed
    /// for root capture when the snapshot is used.
    ///
    /// For example, a stopped server offering `poweron` yields that state and
    /// action.
    pub(in crate::scaleway) fn snapshot_ref(&self) -> InstanceSnapshot {
        InstanceSnapshot {
            id: self.id.clone().into(),
            state: self.state.clone().into(),
            allowed_actions: self
                .allowed_actions
                .iter()
                .cloned()
                .map(Action::from)
                .collect(),
            public_ip: self.public_ip.as_ref().map(|ip| ip.address.clone()),
        }
    }
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
    /// The boot volume is sent explicitly at index `"0"`, so the disk size and
    /// storage type are the resolved plan's rather than a provider default.
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
    /// let instance = backend
    ///     .create_instance_stopped(&request, "img-1", &plan)
    ///     .await?;
    /// assert_eq!(instance.state, "stopped");
    ///
    /// // An instance type the zone does not offer is reported by name.
    /// let err = backend
    ///     .create_instance_stopped(&request, "img-1", &plan)
    ///     .await
    ///     .unwrap_err();
    /// assert!(matches!(err, ScalewayBackendError::InstanceTypeUnavailable { .. }));
    /// ```
    pub(in crate::scaleway) async fn create_instance_stopped(
        &self,
        request: &InstanceRequest,
        image_id: &str,
        plan: &BootVolumePlan,
    ) -> Result<CreatedServer, ScalewayBackendError> {
        let url = format!("{}/zones/{}/servers", self.instance_api_base, request.zone);
        let name = format!("mriya-{}", Uuid::new_v4().simple());
        let tags = Self::instance_tags(self.test_run_id.as_deref());
        let mut volumes = HashMap::new();
        volumes.insert(
            CreateBootVolume::index().to_owned(),
            CreateBootVolume::from_plan(plan),
        );
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
            volumes,
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
            return self.decode_created_server(request, &body).await;
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

/// Reports the boot entry's storage type when the provider sends one.
///
/// Kept here so `create` and the root capture agree on what "the provider
/// reported a type" means.
pub(in crate::scaleway) fn reported_volume_type(volume: &CreatedVolume) -> Option<BootVolumeType> {
    volume
        .volume_type
        .as_deref()
        .and_then(|value| value.parse::<BootVolumeType>().ok())
}
