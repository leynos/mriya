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
        let mut images = if request.project_id.is_empty() {
            ScalewayListInstanceImagesBuilder::new(self.api.clone(), &request.zone)
                .public(true)
                .name(&request.image_label)
                .arch(&request.architecture)
                .run_async()
                .await?
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
            let project_images = scoped.run_async().await?;
            if project_images.is_empty() {
                ScalewayListInstanceImagesBuilder::new(self.api.clone(), &request.zone)
                    .public(true)
                    .name(&request.image_label)
                    .arch(&request.architecture)
                    .run_async()
                    .await?
            } else {
                project_images
            }
        };

        let mut candidates: Vec<_> = Self::filter_images(images, request);

        if candidates.is_empty() {
            return Err(ScalewayBackendError::ImageNotFound {
                label: request.image_label.clone(),
                arch: request.architecture.clone(),
                zone: request.zone.clone(),
            });
        }

        Self::select_image_id(candidates, request)
    }

    pub(super) fn select_image_id(
        mut candidates: Vec<ScalewayImage>,
        request: &InstanceRequest,
    ) -> Result<String, ScalewayBackendError> {
        candidates.sort_by(|lhs, rhs| rhs.creation_date.cmp(&lhs.creation_date));
        Ok(candidates.remove(0).id)
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
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Duration;
    use crate::scaleway::DEFAULT_SSH_PORT;
    use crate::ScalewayConfig;
    use scaleway_rs::ScalewayApi;

    fn snapshot(
        id: &str,
        state: &str,
        allowed: &[&str],
        public_ip: Option<&str>,
    ) -> InstanceSnapshot {
        InstanceSnapshot {
            id: id.to_owned(),
            state: state.to_owned(),
            allowed_actions: allowed
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            public_ip: public_ip.map(str::to_owned),
        }
    }

    fn dummy_config() -> ScalewayConfig {
        ScalewayConfig {
            access_key: None,
            secret_key: String::from("dummy"),
            default_organization_id: None,
            default_project_id: String::from("proj"),
            default_zone: String::from("zone"),
            default_instance_type: String::from("type"),
            default_image: String::from("img"),
            default_architecture: String::from("x86_64"),
        }
    }

    #[tokio::test]
    async fn power_on_if_needed_returns_ok_for_running() {
        let backend = ScalewayBackend {
            api: ScalewayApi::new("dummy"),
            config: dummy_config(),
            ssh_port: DEFAULT_SSH_PORT,
            poll_interval: Duration::from_millis(1),
            wait_timeout: Duration::from_millis(5),
        };
        let snap = snapshot("id", "running", &["poweron"], Some("1.1.1.1"));
        let result = backend.power_on_if_needed("zone", &snap).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn power_on_if_needed_errors_when_not_allowed() {
        let backend = ScalewayBackend {
            api: ScalewayApi::new("dummy"),
            config: dummy_config(),
            ssh_port: DEFAULT_SSH_PORT,
            poll_interval: Duration::from_millis(1),
            wait_timeout: Duration::from_millis(5),
        };
        let snap = snapshot("id", "stopped", &[], None);
        let result = backend.power_on_if_needed("zone", &snap).await;
        assert!(matches!(
            result,
            Err(ScalewayBackendError::PowerOnNotAllowed { .. })
        ));
    }

    /// Minimal backend double to test wait loops without real API calls.
    struct FakeBackend {
        snapshots: VecDeque<Option<InstanceSnapshot>>,
        poll_interval: Duration,
        wait_timeout: Duration,
        ssh_port: u16,
    }

    impl FakeBackend {
        async fn wait_for_public_ip(
            &mut self,
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

                let server_opt = self.snapshots.pop_front().unwrap_or(None);
                let Some(server) = server_opt else {
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

        async fn wait_until_gone(
            &mut self,
            handle: &InstanceHandle,
        ) -> Result<(), ScalewayBackendError> {
            let deadline = Instant::now() + self.wait_timeout;
            while Instant::now() <= deadline {
                let next = self.snapshots.pop_front().unwrap_or(None);
                if next.is_none() {
                    return Ok(());
                }
                sleep(self.poll_interval).await;
            }
            Err(ScalewayBackendError::ResidualResource {
                instance_id: handle.id.clone(),
            })
        }
    }

    #[tokio::test]
    async fn wait_for_public_ip_returns_missing_ip() {
        let mut fake = FakeBackend {
            snapshots: VecDeque::from(vec![
                Some(snapshot("id", "running", &[], None)),
                Some(snapshot("id", "running", &[], None)),
            ]),
            poll_interval: Duration::from_millis(1),
            wait_timeout: Duration::from_millis(5),
            ssh_port: DEFAULT_SSH_PORT,
        };
        let handle = InstanceHandle {
            id: "id".to_owned(),
            zone: "zone".to_owned(),
        };
        let result = fake.wait_for_public_ip(&handle).await;
        assert!(
            matches!(
                result,
                Err(ScalewayBackendError::MissingPublicIp { .. }
                    | ScalewayBackendError::Timeout { .. })
            ),
            "unexpected wait_for_public_ip outcome: {result:?}"
        );
    }

    #[tokio::test]
    async fn wait_until_gone_times_out_on_residual() {
        let mut fake = FakeBackend {
            snapshots: VecDeque::from(vec![Some(snapshot("id", "running", &[], None))]),
            poll_interval: Duration::from_millis(1),
            wait_timeout: Duration::from_millis(2),
            ssh_port: DEFAULT_SSH_PORT,
        };
        let handle = InstanceHandle {
            id: "id".to_owned(),
            zone: "zone".to_owned(),
        };
        let result = fake.wait_until_gone(&handle).await;
        assert!(matches!(
            result,
            Err(ScalewayBackendError::ResidualResource { .. })
        ));
    }
}
