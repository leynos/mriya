//! Instances API public IP reads, listings, release, and presence checks.

use super::{
    HTTP_CLIENT,
    http::{LIST_PAGE_SIZE, checked_json, checked_response, collect_pages},
    models::{IpList, IpResponse, Presence, PublicIpModel},
};
use crate::{
    ownership::ResourceRef,
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

impl ScalewayBackend {
    /// Reads one public IP allocation.
    ///
    /// # Errors
    ///
    /// Returns a classified provider error; a 404 becomes
    /// [`ScalewayBackendError::NotFound`].
    pub(in crate::scaleway) async fn get_ip(
        &self,
        zone: &str,
        ip_id: &str,
    ) -> Result<PublicIpModel, ScalewayBackendError> {
        let url = self.instance_url(zone, &format!("ips/{ip_id}"));
        let response: IpResponse =
            checked_json("get ip", self.authorized(HTTP_CLIENT.get(&url))).await?;
        Ok(response.ip)
    }

    /// Lists the public IP allocations held by `project_id` in `zone`.
    ///
    /// # Errors
    ///
    /// Returns the first page failure, or a provider error when pagination does
    /// not terminate.
    pub(in crate::scaleway) async fn list_ips(
        &self,
        zone: &str,
        project_id: &str,
    ) -> Result<Vec<PublicIpModel>, ScalewayBackendError> {
        collect_pages("list ips", |page| {
            self.fetch_ip_page(zone, project_id, page)
        })
        .await
    }

    /// Fetches one page of [`ScalewayBackend::list_ips`].
    async fn fetch_ip_page(
        &self,
        zone: &str,
        project_id: &str,
        page: u32,
    ) -> Result<Vec<PublicIpModel>, ScalewayBackendError> {
        let url = self.instance_url(zone, "ips");
        let page_text = page.to_string();
        let request = self.authorized(HTTP_CLIENT.get(&url)).query(&[
            ("project", project_id),
            ("per_page", &LIST_PAGE_SIZE.to_string()),
            ("page", &page_text),
        ]);

        let response: IpList = checked_json("list ips", request).await?;
        Ok(response.ips)
    }

    /// Releases a public IP allocation.
    ///
    /// # Errors
    ///
    /// Returns a classified provider error; the caller verifies absence.
    pub(in crate::scaleway) async fn delete_ip(
        &self,
        zone: &str,
        ip_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        let url = self.instance_url(zone, &format!("ips/{ip_id}"));
        checked_response("delete ip", self.authorized(HTTP_CLIENT.delete(&url))).await?;
        Ok(())
    }

    /// Checks whether a recorded public IP still exists.
    ///
    /// A dynamic address attached to a server is released when the server is
    /// deleted, so absence here is expected after a successful server
    /// deletion; a non-dynamic address is deleted explicitly.
    ///
    /// # Errors
    ///
    /// Returns the provider failure that prevented the check.
    pub(in crate::scaleway) async fn ip_presence(
        &self,
        resource: &ResourceRef,
        project_id: &str,
    ) -> Result<Presence<PublicIpModel>, ScalewayBackendError> {
        let zone = resource.zone();
        match self.get_ip(zone, resource.id()).await {
            Ok(ip) if ip.id == resource.id() => Ok(Presence::Present(ip)),
            Ok(ip) => Err(ScalewayBackendError::Provider {
                message: format!(
                    "get ip returned {}, but {} was requested",
                    ip.id,
                    resource.id()
                ),
            }),
            Err(ScalewayBackendError::NotFound { .. }) => {
                let ips = self.list_ips(zone, project_id).await?;
                Ok(ips
                    .into_iter()
                    .find(|ip| ip.id == resource.id())
                    .map_or(Presence::ConfirmedAbsent, Presence::Present))
            }
            Err(error) => Err(error),
        }
    }
}
