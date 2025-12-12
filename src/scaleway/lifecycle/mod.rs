//! Instance lifecycle helpers for the Scaleway backend.

use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use crate::backend::{InstanceHandle, InstanceNetworking, InstanceRequest};
use scaleway_rs::{ScalewayImage, ScalewayListInstanceImagesBuilder};
use tokio::time::sleep;

use super::volume::{UpdateInstanceVolumesRequest, VolumeAttachment};
use super::{ScalewayBackend, ScalewayBackendError};
use crate::scaleway::types::{Action, InstanceId, InstanceState, Zone};

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceSnapshot {
    pub(crate) id: InstanceId,
    pub(crate) state: InstanceState,
    pub(crate) allowed_actions: Vec<Action>,
    pub(crate) public_ip: Option<String>,
}

impl ScalewayBackend {
    #[expect(
        clippy::excessive_nesting,
        reason = "organisation scoping requires nested builder updates before execution"
    )]
    pub(super) async fn resolve_image_id(
        &self,
        request: &InstanceRequest,
    ) -> Result<String, ScalewayBackendError> {
        self.resolve_image_id_with(
            request,
            || async move {
                if request.project_id.is_empty() {
                    Ok(Vec::new())
                } else {
                    let mut scoped =
                        ScalewayListInstanceImagesBuilder::new(self.api.clone(), &request.zone)
                            .public(true)
                            .project(&request.project_id)
                            .name(&request.image_label)
                            .arch(&request.architecture);
                    if let Some(org) = &request.organisation_id {
                        scoped = scoped.organization(org);
                    }
                    scoped.run_async().await.map_err(ScalewayBackendError::from)
                }
            },
            || async move {
                ScalewayListInstanceImagesBuilder::new(self.api.clone(), &request.zone)
                    .public(true)
                    .name(&request.image_label)
                    .arch(&request.architecture)
                    .run_async()
                    .await
                    .map_err(ScalewayBackendError::from)
            },
        )
        .await
    }

    pub(super) async fn resolve_image_id_with<FutA, FutB, FetchA, FetchB>(
        &self,
        request: &InstanceRequest,
        project_fetch: FetchA,
        public_fetch: FetchB,
    ) -> Result<String, ScalewayBackendError>
    where
        FetchA: FnOnce() -> FutA,
        FetchB: FnOnce() -> FutB,
        FutA: Future<Output = Result<Vec<ScalewayImage>, ScalewayBackendError>>,
        FutB: Future<Output = Result<Vec<ScalewayImage>, ScalewayBackendError>>,
    {
        let project_images = project_fetch().await?;

        let public_images = if project_images.is_empty() {
            public_fetch().await?
        } else {
            Vec::new()
        };

        Self::select_image_from_sources(project_images, public_images, request)
    }

    pub(super) fn select_image_id(
        mut candidates: Vec<ScalewayImage>,
        request: &InstanceRequest,
    ) -> Result<String, ScalewayBackendError> {
        if candidates.is_empty() {
            return Err(ScalewayBackendError::ImageNotFound {
                label: request.image_label.clone(),
                arch: request.architecture.clone(),
                zone: request.zone.clone(),
            });
        }
        candidates.sort_by(|lhs, rhs| rhs.creation_date.cmp(&lhs.creation_date));
        Ok(candidates.remove(0).id)
    }

    pub(super) fn select_image_from_sources(
        project_images: Vec<ScalewayImage>,
        public_images: Vec<ScalewayImage>,
        request: &InstanceRequest,
    ) -> Result<String, ScalewayBackendError> {
        let primary = if project_images.is_empty() {
            public_images
        } else {
            project_images
        };

        let candidates = Self::filter_images(primary, request);

        if candidates.is_empty() {
            return Err(ScalewayBackendError::ImageNotFound {
                label: request.image_label.clone(),
                arch: request.architecture.clone(),
                zone: request.zone.clone(),
            });
        }

        Self::select_image_id(candidates, request)
    }

    pub(super) fn filter_images(
        images: Vec<ScalewayImage>,
        request: &InstanceRequest,
    ) -> Vec<ScalewayImage> {
        images
            .into_iter()
            .filter(|image| image.arch == request.architecture)
            .filter(|image| image.state == "available")
            .collect()
    }

    pub(super) async fn power_on_if_needed(
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

    pub(super) async fn fetch_instance(
        &self,
        handle: &InstanceHandle,
    ) -> Result<Option<InstanceSnapshot>, ScalewayBackendError> {
        let mut servers = self
            .api
            .list_instances(&handle.zone)
            .servers(&handle.id)
            .per_page(1)
            .run_async()
            .await?;

        Ok(servers.pop().map(|server| InstanceSnapshot {
            id: server.id.into(),
            state: server.state.into(),
            allowed_actions: server
                .allowed_actions
                .into_iter()
                .map(Action::from)
                .collect(),
            public_ip: server.public_ip.map(|ip| ip.address),
        }))
    }

    pub(super) async fn wait_for_public_ip(
        &self,
        handle: &InstanceHandle,
    ) -> Result<InstanceNetworking, ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            if Instant::now() > deadline {
                return Err(ScalewayBackendError::Timeout {
                    action: "wait_for_ready".to_owned(),
                    instance_id: handle.id.clone(),
                });
            }

            let Some(server) = self.fetch_instance(handle).await? else {
                sleep(self.poll_interval).await;
                continue;
            };

            if server.state.as_str() != "running" {
                sleep(self.poll_interval).await;
                continue;
            }

            if let Some(address) = server
                .public_ip
                .as_ref()
                .and_then(|ip| IpAddr::from_str(ip).ok())
            {
                return Ok(InstanceNetworking {
                    public_ip: address,
                    ssh_port: self.ssh_port,
                });
            }

            if Instant::now() > deadline {
                return Err(ScalewayBackendError::MissingPublicIp {
                    instance_id: handle.id.clone(),
                });
            }

            sleep(self.poll_interval).await;
        }
    }

    pub(super) async fn wait_until_gone(
        &self,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        while Instant::now() <= deadline {
            if self.fetch_instance(handle).await?.is_none() {
                return Ok(());
            }
            sleep(self.poll_interval).await;
        }

        Err(ScalewayBackendError::ResidualResource {
            instance_id: handle.id.clone(),
        })
    }

    /// Attaches a volume to a stopped instance.
    ///
    /// The volume must be in the same zone as the instance. The attachment
    /// uses a direct HTTP PATCH call since the `scaleway-rs` crate does not
    /// expose volume management in its instance builder.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeAttachmentFailed`] when the API
    /// rejects the attachment request. Returns
    /// [`ScalewayBackendError::VolumeNotFound`] when the root volume is missing
    /// from the instance snapshot used to build the attachment payload.
    pub(super) async fn attach_volume(
        &self,
        handle: &InstanceHandle,
        volume_id: &str,
        root_volume_id: String,
    ) -> Result<(), ScalewayBackendError> {
        if root_volume_id.trim().is_empty() {
            return Err(ScalewayBackendError::VolumeNotFound {
                volume_id: String::from("0"),
                zone: handle.zone.clone(),
            });
        }

        let mut volumes = HashMap::new();

        // Preserve root volume at index "0"
        volumes.insert(
            String::from("0"),
            VolumeAttachment {
                id: root_volume_id,
                boot: true,
            },
        );

        // Add cache volume at index "1"
        volumes.insert(
            String::from("1"),
            VolumeAttachment {
                id: volume_id.to_owned(),
                boot: false,
            },
        );

        let request = UpdateInstanceVolumesRequest { volumes };
        self.patch_instance_volumes(handle, &request).await
    }

    /// Sends a PATCH request to update instance volumes.
    async fn patch_instance_volumes(
        &self,
        handle: &InstanceHandle,
        request: &UpdateInstanceVolumesRequest,
    ) -> Result<(), ScalewayBackendError> {
        let url = format!(
            "https://api.scaleway.com/instance/v1/zones/{}/servers/{}",
            handle.zone, handle.id
        );

        let response = HTTP_CLIENT
            .patch(&url)
            .header("X-Auth-Token", &self.config.secret_key)
            .json(request)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|err| ScalewayBackendError::Provider {
                message: err.to_string(),
            })?;

        if response.status().is_success() {
            return Ok(());
        }

        let error_text = response.text().await.unwrap_or_default();
        let volume_id = request
            .volumes
            .get("1")
            .map_or_else(String::new, |v| v.id.clone());

        Err(ScalewayBackendError::VolumeAttachmentFailed {
            volume_id,
            instance_id: handle.id.clone(),
            message: error_text,
        })
    }
}

#[cfg(test)]
mod tests;
