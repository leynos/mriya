//! Image resolution helpers for the Scaleway backend.

use std::future::Future;

use crate::backend::InstanceRequest;
use scaleway_rs::{ScalewayImage, ScalewayListInstanceImagesBuilder};

use super::super::{ScalewayBackend, ScalewayBackendError};

impl ScalewayBackend {
    /// Resolves the image to boot from, preferring the project's own images.
    ///
    /// Public images are queried only when the project-scoped lookup is empty.
    /// With an empty project identifier the project lookup is skipped.
    ///
    /// # Errors
    ///
    /// Returns `ImageNotFound` when no matching available image exists in the
    /// selected source, or a provider error when a lookup fails.
    ///
    /// The selected source is the project's image list when it is non-empty
    /// and the public list otherwise. A project list holding only unavailable
    /// images counts as non-empty, so the public list is never consulted and a
    /// matching public image yields `ImageNotFound` rather than a match. This
    /// mirrors `select_image_from_sources`, which never merges the two lists.
    #[expect(
        clippy::excessive_nesting,
        reason = "organization scoping requires nested builder updates before execution"
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

    /// Resolves an image identifier from two injected fetchers.
    ///
    /// `project_fetch` runs first; `public_fetch` runs only if it returned no
    /// images. Injecting the fetchers lets tests exercise the fallback order
    /// without calling the provider.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // An empty project list falls through to the public list.
    /// let id = backend
    ///     .resolve_image_id_with(&request, || async { Ok(Vec::new()) }, || async {
    ///         Ok(vec![image("public-id", "2026-01-01T00:00:00Z")])
    ///     })
    ///     .await?;
    /// assert_eq!(id, "public-id");
    ///
    /// // A non-empty project list is used as-is; the public fetcher never runs.
    /// let id = backend
    ///     .resolve_image_id_with(
    ///         &request,
    ///         || async { Ok(vec![image("project-id", "2026-01-01T00:00:00Z")]) },
    ///         || async { panic!("public list must not be queried") },
    ///     )
    ///     .await?;
    /// assert_eq!(id, "project-id");
    /// ```
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

    /// Picks the candidate with the latest `creation_date`.
    ///
    /// # Errors
    ///
    /// Returns `ImageNotFound` when `candidates` is empty.
    ///
    /// # Examples
    ///
    /// The example is marked `ignore` because the helper is crate-internal and
    /// so is unreachable from a doctest; the equivalent assertions run as unit
    /// tests in `super::tests`.
    ///
    /// ```ignore
    /// let matching = image("11111111-1111-1111-1111-111111111111", "2026-01-01T00:00:00Z");
    /// let newer = image("22222222-2222-2222-2222-222222222222", "2026-06-01T00:00:00Z");
    ///
    /// // The newest candidate wins regardless of input order.
    /// let selected = select_image_id(vec![newer, matching], &request)?;
    /// assert_eq!(selected, "22222222-2222-2222-2222-222222222222");
    ///
    /// // An empty candidate list is an error naming the requested image.
    /// let err = select_image_id(Vec::new(), &request).unwrap_err();
    /// assert!(matches!(err, ScalewayBackendError::ImageNotFound { .. }));
    /// ```
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
        candidates.sort_by(|lhs, rhs| lhs.creation_date.cmp(&rhs.creation_date));
        let selected = candidates
            .pop()
            .ok_or_else(|| ScalewayBackendError::ImageNotFound {
                label: request.image_label.clone(),
                arch: request.architecture.clone(),
                zone: request.zone.clone(),
            })?;
        Ok(selected.id)
    }

    /// Filters then selects from the project images, or from the public images
    /// when there are no project images.
    ///
    /// The public list is never merged with a non-empty project list.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // No project images: the public list supplies the candidate.
    /// let id = select_image_from_sources(Vec::new(), vec![public], &request)?;
    /// assert_eq!(id, public.id);
    ///
    /// // A non-empty project list wins even when the public list would match.
    /// let id = select_image_from_sources(vec![project], vec![public], &request)?;
    /// assert_eq!(id, project.id);
    /// ```
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

    /// Keeps images whose architecture matches the request and whose state is
    /// `available`.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Wrong architecture and non-available images are dropped.
    /// let kept = filter_images(
    ///     vec![
    ///         image("ok", "2026-01-01T00:00:00Z"),
    ///         with_state("starting", image("booting", "2026-01-01T00:00:00Z")),
    ///         with_arch("arm64", image("other-arch", "2026-01-01T00:00:00Z")),
    ///     ],
    ///     &request,
    /// );
    /// assert_eq!(kept.len(), 1);
    /// assert_eq!(kept[0].id, "ok");
    /// ```
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
