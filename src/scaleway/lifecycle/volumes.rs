//! Instances API volume reads, listings, deletion, presence, and detachment.

use std::collections::HashMap;

use super::{
    HTTP_CLIENT,
    http::{LIST_PAGE_SIZE, ListQuery, checked_json, checked_response, collect_pages},
    models::{InstanceVolumeList, InstanceVolumeResponse, Presence, VolumeModel},
    volume_attach::{VolumePatchContext, patch_instance_volumes},
};
use crate::{
    backend::InstanceHandle,
    ownership::ResourceRef,
    scaleway::{
        ScalewayBackend,
        ScalewayBackendError,
        volume::{UpdateInstanceVolumesRequest, VolumeAttachment},
    },
};

impl ScalewayBackend {
    /// Reads one Instances API volume.
    ///
    /// # Errors
    ///
    /// Returns a classified provider error; a 404 becomes
    /// [`ScalewayBackendError::NotFound`].
    pub(in crate::scaleway) async fn get_instance_volume(
        &self,
        zone: &str,
        volume_id: &str,
    ) -> Result<VolumeModel, ScalewayBackendError> {
        let url = self.instance_url(zone, &format!("volumes/{volume_id}"));
        let response: InstanceVolumeResponse =
            checked_json("get volume", self.authorized(HTTP_CLIENT.get(&url))).await?;
        Ok(response.volume)
    }

    /// Lists Instances API volumes in `zone`, optionally keeping only those
    /// tagged `tag`. Pages are collected first and filtered locally
    /// afterwards, so a page with no matching volume cannot end pagination.
    ///
    /// # Errors
    ///
    /// Returns the first page failure, or a provider error when pagination does
    /// not terminate.
    pub(in crate::scaleway) async fn list_instance_volumes(
        &self,
        zone: &str,
        project_id: &str,
        tag: Option<&str>,
    ) -> Result<Vec<VolumeModel>, ScalewayBackendError> {
        let query = ListQuery {
            zone,
            project_id,
            tag,
        };
        let page_items = collect_pages("list volumes", |page| {
            self.fetch_instance_volume_page(query, page)
        })
        .await?;
        Ok(page_items
            .into_iter()
            .filter(|volume| volume.zone.trim().is_empty() || volume.zone == zone)
            .filter(|volume| tag.is_none_or(|wanted| volume.tags.iter().any(|t| t == wanted)))
            .collect())
    }

    /// Fetches one page of [`ScalewayBackend::list_instance_volumes`].
    async fn fetch_instance_volume_page(
        &self,
        query: ListQuery<'_>,
        page: u32,
    ) -> Result<Vec<VolumeModel>, ScalewayBackendError> {
        let url = self.instance_url(query.zone, "volumes");
        let page_text = page.to_string();
        let mut request = self.authorized(HTTP_CLIENT.get(&url)).query(&[
            ("project", query.project_id),
            ("per_page", &LIST_PAGE_SIZE.to_string()),
            ("page", &page_text),
        ]);
        if let Some(filter) = query.tag {
            request = request.query(&[("tags", filter)]);
        }

        let response: InstanceVolumeList = checked_json("list volumes", request).await?;
        Ok(response.volumes)
    }

    /// Deletes an Instances API volume.
    ///
    /// # Errors
    ///
    /// Returns a classified provider error; the caller verifies absence.
    pub(in crate::scaleway) async fn delete_instance_volume(
        &self,
        zone: &str,
        volume_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        let url = self.instance_url(zone, &format!("volumes/{volume_id}"));
        checked_response("delete volume", self.authorized(HTTP_CLIENT.delete(&url))).await?;
        Ok(())
    }

    /// Checks whether a recorded Instances API volume still exists.
    ///
    /// # Errors
    ///
    /// Returns the provider failure that prevented the check.
    pub(in crate::scaleway) async fn instance_volume_presence(
        &self,
        resource: &ResourceRef,
        project_id: &str,
    ) -> Result<Presence<VolumeModel>, ScalewayBackendError> {
        let zone = resource.zone();
        match self.get_instance_volume(zone, resource.id()).await {
            Ok(volume) if volume.id == resource.id() => {
                ensure_volume_zone(&volume, zone)?;
                Ok(Presence::Present(volume))
            }
            Ok(volume) => Err(ScalewayBackendError::Provider {
                message: format!(
                    "get volume returned {}, but {} was requested",
                    volume.id,
                    resource.id()
                ),
            }),
            Err(ScalewayBackendError::NotFound { .. }) => {
                // Unfiltered on purpose: a row whose ID matches but whose zone
                // contradicts the record must be an error, not silently gone.
                let volumes = self
                    .list_instance_volumes_unfiltered(zone, project_id)
                    .await?;
                let Some(volume) = volumes
                    .into_iter()
                    .find(|volume| volume.id == resource.id())
                else {
                    return Ok(Presence::ConfirmedAbsent);
                };
                ensure_volume_zone(&volume, zone)?;
                Ok(Presence::Present(volume))
            }
            Err(error) => Err(error),
        }
    }

    /// Lists Instances API volumes without the local zone or tag filters.
    async fn list_instance_volumes_unfiltered(
        &self,
        zone: &str,
        project_id: &str,
    ) -> Result<Vec<VolumeModel>, ScalewayBackendError> {
        let query = ListQuery {
            zone,
            project_id,
            tag: None,
        };
        collect_pages("list volumes", |page| {
            self.fetch_instance_volume_page(query, page)
        })
        .await
    }

    /// Attaches a volume to a stopped instance, preserving its root volume.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeNotFound`] when `root_volume_id`
    /// is blank, and [`ScalewayBackendError::VolumeAttachmentFailed`] when the
    /// provider rejects the update.
    pub(in crate::scaleway) async fn attach_volume(
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
        volumes.insert(
            String::from("0"),
            VolumeAttachment {
                id: root_volume_id,
                boot: true,
            },
        );
        volumes.insert(
            String::from("1"),
            VolumeAttachment {
                id: volume_id.to_owned(),
                boot: false,
            },
        );

        let request = UpdateInstanceVolumesRequest { volumes };
        patch_instance_volumes(
            self,
            handle,
            &request,
            VolumePatchContext::attach(volume_id),
        )
        .await
    }

    /// Detaches `volume_ids` from the instance and verifies they are gone.
    ///
    /// The replacement volume map keeps every other volume the server reports,
    /// because the Instances API replaces the whole map rather than merging
    /// into it. Verification re-reads the server and fails when any requested
    /// identifier is still attached.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::VolumeDetachFailed`] when the provider
    /// rejects the update or the volume is still attached afterwards.
    pub(in crate::scaleway) async fn detach_volumes(
        &self,
        handle: &InstanceHandle,
        volume_ids: &[String],
    ) -> Result<(), ScalewayBackendError> {
        if volume_ids.is_empty() {
            return Ok(());
        }

        let server = self.get_server(&handle.zone, &handle.id).await?;
        let mut volumes = HashMap::new();
        for (index, volume) in &server.volumes {
            if volume_ids.iter().any(|id| id == &volume.id) {
                continue;
            }
            volumes.insert(
                index.clone(),
                VolumeAttachment {
                    id: volume.id.clone(),
                    boot: index == "0",
                },
            );
        }

        let first = volume_ids.first().map_or("", String::as_str);
        let request = UpdateInstanceVolumesRequest { volumes };
        patch_instance_volumes(self, handle, &request, VolumePatchContext::detach(first)).await?;

        let refreshed = self.get_server(&handle.zone, &handle.id).await?;
        if let Some(remaining) = refreshed
            .volumes
            .values()
            .find(|volume| volume_ids.iter().any(|id| id == &volume.id))
        {
            return Err(ScalewayBackendError::VolumeDetachFailed {
                volume_id: remaining.id.clone(),
                instance_id: handle.id.clone(),
                message: String::from("volume is still attached after the update"),
            });
        }

        Ok(())
    }
}

/// Rejects a volume whose reported zone contradicts the record.
///
/// An omitted zone is accepted, because the request URL already scopes the
/// call; a contradicting one means the identifier names a different volume.
fn ensure_volume_zone(
    volume: &VolumeModel,
    expected_zone: &str,
) -> Result<(), ScalewayBackendError> {
    if volume.zone.trim().is_empty() || volume.zone == expected_zone {
        return Ok(());
    }
    Err(ScalewayBackendError::Provider {
        message: format!(
            "volume {} is in zone {zone}, but {expected_zone} was requested",
            volume.id,
            zone = volume.zone
        ),
    })
}
