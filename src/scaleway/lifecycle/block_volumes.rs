//! Block Storage API volume reads, listings, deletion, and idle waiting.

use tokio::time::{Instant, sleep};

use super::{
    HTTP_CLIENT,
    http::{LIST_PAGE_SIZE, ListQuery, checked_json, checked_response, collect_pages},
    models::{BlockVolumeList, BlockVolumeModel, BlockVolumeResponse, Presence},
};
use crate::{
    ownership::ResourceRef,
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

impl ScalewayBackend {
    /// Reads one Block Storage volume.
    ///
    /// # Errors
    ///
    /// Returns a classified provider error; a 404 becomes
    /// [`ScalewayBackendError::NotFound`].
    pub(in crate::scaleway) async fn get_block_volume(
        &self,
        zone: &str,
        volume_id: &str,
    ) -> Result<BlockVolumeModel, ScalewayBackendError> {
        let url = self.block_url(zone, &format!("volumes/{volume_id}"));
        let response: BlockVolumeResponse =
            checked_json("get block volume", self.authorized(HTTP_CLIENT.get(&url))).await?;
        Ok(response.volume)
    }

    /// Lists Block Storage volumes in `zone`, optionally keeping only those
    /// tagged `tag`. Pages are collected first and filtered locally
    /// afterwards, so a page with no matching volume cannot end pagination.
    ///
    /// # Errors
    ///
    /// Returns the first page failure, or a provider error when pagination does
    /// not terminate.
    pub(in crate::scaleway) async fn list_block_volumes(
        &self,
        zone: &str,
        project_id: &str,
        tag: Option<&str>,
    ) -> Result<Vec<BlockVolumeModel>, ScalewayBackendError> {
        let query = ListQuery {
            zone,
            project_id,
            tag,
        };
        let page_items = collect_pages("list block volumes", |page| {
            self.fetch_block_volume_page(query, page)
        })
        .await?;
        Ok(page_items
            .into_iter()
            .filter(|volume| volume.zone.trim().is_empty() || volume.zone == zone)
            .filter(|volume| volume.project_id.trim().is_empty() || volume.project_id == project_id)
            .filter(|volume| tag.is_none_or(|wanted| volume.tags.iter().any(|t| t == wanted)))
            .collect())
    }

    /// Fetches one page of [`ScalewayBackend::list_block_volumes`].
    async fn fetch_block_volume_page(
        &self,
        query: ListQuery<'_>,
        page: u32,
    ) -> Result<Vec<BlockVolumeModel>, ScalewayBackendError> {
        let url = self.block_url(query.zone, "volumes");
        let page_text = page.to_string();
        let mut request = self.authorized(HTTP_CLIENT.get(&url)).query(&[
            ("project_id", query.project_id),
            ("page_size", &LIST_PAGE_SIZE.to_string()),
            ("page", &page_text),
        ]);
        if let Some(filter) = query.tag {
            request = request.query(&[("tags", filter)]);
        }

        let response: BlockVolumeList = checked_json("list block volumes", request).await?;
        Ok(response.volumes)
    }

    /// Deletes a Block Storage volume.
    ///
    /// # Errors
    ///
    /// Returns a classified provider error; the caller verifies absence.
    pub(in crate::scaleway) async fn delete_block_volume(
        &self,
        zone: &str,
        volume_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        let url = self.block_url(zone, &format!("volumes/{volume_id}"));
        checked_response(
            "delete block volume",
            self.authorized(HTTP_CLIENT.delete(&url)),
        )
        .await?;
        Ok(())
    }

    /// Checks whether a recorded Block Storage volume still exists.
    ///
    /// # Errors
    ///
    /// Returns the provider failure that prevented the check.
    pub(in crate::scaleway) async fn block_volume_presence(
        &self,
        resource: &ResourceRef,
        project_id: &str,
    ) -> Result<Presence<BlockVolumeModel>, ScalewayBackendError> {
        let zone = resource.zone();
        match self.get_block_volume(zone, resource.id()).await {
            Ok(volume) if volume.id == resource.id() => Ok(Presence::Present(volume)),
            Ok(volume) => Err(ScalewayBackendError::Provider {
                message: format!(
                    "get block volume returned {}, but {} was requested",
                    volume.id,
                    resource.id()
                ),
            }),
            Err(ScalewayBackendError::NotFound { .. }) => {
                let volumes = self.list_block_volumes(zone, project_id, None).await?;
                Ok(volumes
                    .into_iter()
                    .find(|volume| volume.id == resource.id())
                    .map_or(Presence::ConfirmedAbsent, Presence::Present))
            }
            Err(error) => Err(error),
        }
    }

    /// Waits until a Block Storage volume is no longer `in_use`.
    ///
    /// Deleting a volume that an instance still references is rejected by the
    /// provider, so cleanup waits for the reference to be released first. A
    /// volume that has already disappeared needs no wait.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeTimeout`] when the volume remains
    /// `in_use` through the backend's wait deadline, and any provider failure
    /// other than a 404.
    pub(in crate::scaleway) async fn wait_for_block_volume_idle(
        &self,
        zone: &str,
        volume_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            match self.get_block_volume(zone, volume_id).await {
                Ok(volume) if !volume.is_in_use() => return Ok(()),
                Ok(_) => {}
                Err(ScalewayBackendError::NotFound { .. }) => return Ok(()),
                Err(error) => return Err(error),
            }

            if Instant::now() > deadline {
                return Err(ScalewayBackendError::VolumeTimeout {
                    action: String::from("wait for the volume to leave in_use"),
                    volume_id: volume_id.to_owned(),
                });
            }
            sleep(self.poll_interval).await;
        }
    }
}
