//! Instance lifecycle helpers for the Scaleway backend.

use std::net::IpAddr;
use std::str::FromStr;
use std::time::Instant;

use crate::backend::{InstanceHandle, InstanceNetworking, InstanceRequest};
use scaleway_rs::{ScalewayImage, ScalewayListInstanceImagesBuilder};
use tokio::time::sleep;

use super::{ScalewayBackend, ScalewayBackendError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceSnapshot {
    pub(crate) id: String,
    pub(crate) state: String,
    pub(crate) allowed_actions: Vec<String>,
    pub(crate) public_ip: Option<String>,
}

impl ScalewayBackend {
    pub(super) async fn resolve_image_id(
        &self,
        request: &InstanceRequest,
    ) -> Result<String, ScalewayBackendError> {
        let project_images = if request.project_id.is_empty() {
            Vec::new()
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
            scoped.run_async().await?
        };

        let public_images = if project_images.is_empty() {
            ScalewayListInstanceImagesBuilder::new(self.api.clone(), &request.zone)
                .public(true)
                .name(&request.image_label)
                .arch(&request.architecture)
                .run_async()
                .await?
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
        zone: &str,
        snapshot: &InstanceSnapshot,
    ) -> Result<(), ScalewayBackendError> {
        if snapshot.state == "running" {
            return Ok(());
        }

        if snapshot
            .allowed_actions
            .iter()
            .any(|action| action == "poweron")
        {
            self.api
                .perform_instance_action_async(zone, &snapshot.id, "poweron")
                .await?;
            return Ok(());
        }

        Err(ScalewayBackendError::PowerOnNotAllowed {
            instance_id: snapshot.id.clone(),
            state: snapshot.state.clone(),
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
            id: server.id,
            state: server.state,
            allowed_actions: server.allowed_actions,
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

            if server.state != "running" {
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
}

#[cfg(test)]
mod tests;
