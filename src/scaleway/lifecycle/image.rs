//! Image resolution helpers for the Scaleway backend.

use std::future::Future;

use crate::backend::InstanceRequest;
use scaleway_rs::{ScalewayImage, ScalewayListInstanceImagesBuilder};

use super::super::{ScalewayBackend, ScalewayBackendError};

impl ScalewayBackend {
    #[expect(
        clippy::excessive_nesting,
        reason = "organisation scoping requires nested builder updates before execution"
    )]
    pub(in crate::scaleway) async fn resolve_image_id(
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

    pub(in crate::scaleway) async fn resolve_image_id_with<FutA, FutB, FetchA, FetchB>(
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

    pub(in crate::scaleway) fn select_image_id(
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

    pub(in crate::scaleway) fn select_image_from_sources(
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

        Self::select_image_id(candidates, request)
    }

    pub(in crate::scaleway) fn filter_images(
        images: Vec<ScalewayImage>,
        request: &InstanceRequest,
    ) -> Vec<ScalewayImage> {
        images
            .into_iter()
            .filter(|image| image.arch == request.architecture)
            .filter(|image| image.state == "available")
            .collect()
    }
}
