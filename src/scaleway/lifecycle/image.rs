//! Image resolution helpers for the Scaleway backend.

use std::future::Future;

use scaleway_rs::{ScalewayImage, ScalewayListInstanceImagesBuilder};

use super::super::{ScalewayBackend, ScalewayBackendError};
use crate::{
    backend::InstanceRequest,
    boot_volume::{BootVolumeType, ImageStorage},
};

/// Maps an image's own root-volume type onto the storage kinds a boot plan can
/// use.
///
/// Scaleway publishes six identifiers and only some can back a new boot
/// volume. Anything else is reported by name rather than assumed compatible.
///
/// For example, `l_ssd` yields local only, `sbs_volume` and `sbs_snapshot`
/// yield SBS only, and `unified` yields both.
///
/// # Errors
///
/// Returns the observed identifier in the error for `b_ssd`, `scratch`, and
/// any unrecognised value.
pub(in crate::scaleway) fn image_storage_from_type(
    volume_type: &str,
) -> Result<ImageStorage, String> {
    match volume_type {
        "l_ssd" => Ok(ImageStorage::new(true, false)),
        "sbs_volume" | "sbs_snapshot" => Ok(ImageStorage::new(false, true)),
        "unified" => Ok(ImageStorage::new(true, true)),
        other => Err(other.to_owned()),
    }
}

/// The image root facts a boot decision needs.
///
/// # Examples
///
/// ```ignore
/// let root = ImageRoot {
///     id: String::from("img-1"),
///     size_bytes: 10_000_000_000,
///     storage: ImageStorage::new(false, true),
///     volume_type: String::from("sbs_snapshot"),
/// };
/// assert!(root.storage.sbs);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::scaleway) struct ImageRoot {
    /// Provider identifier of the selected image.
    pub(crate) id: String,
    /// Size of the image's own root volume, in bytes.
    pub(crate) size_bytes: u64,
    /// Storage kinds the image can be booted from.
    pub(crate) storage: ImageStorage,
    /// The identifier the provider reported, kept for error messages.
    pub(crate) volume_type: String,
}

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
    ) -> Result<ImageRoot, ScalewayBackendError> {
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
    ) -> Result<ImageRoot, ScalewayBackendError>
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
    ) -> Result<ImageRoot, ScalewayBackendError> {
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
        Self::image_root(selected, request)
    }

    /// Turns the selected image into the root facts a boot plan needs.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::ImageStorageIncompatible`] listing the
    /// observed root identifier when the image cannot back any boot volume.
    ///
    /// For example, an `l_ssd` image yields local-only storage, while a
    /// `scratch` image is refused by name.
    fn image_root(
        image: ScalewayImage,
        request: &InstanceRequest,
    ) -> Result<ImageRoot, ScalewayBackendError> {
        let volume_type = image.root_volume.volume_type.clone();
        match image_storage_from_type(&volume_type) {
            Ok(storage) => Ok(ImageRoot {
                id: image.id,
                size_bytes: image.root_volume.size,
                storage,
                volume_type,
            }),
            Err(observed) => Err(ScalewayBackendError::ImageStorageIncompatible {
                label: request.image_label.clone(),
                arch: request.architecture.clone(),
                zone: request.zone.clone(),
                observed,
                requested: request.boot_volume.volume_type,
            }),
        }
    }

    /// Filters then selects from the project images, or from the public images
    /// when there are no project images.
    ///
    /// The public list is never merged with a non-empty project list.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // No project images: the public image supplies the winner. Its ID is
    /// // captured first because the vector takes ownership of the image.
    /// let public_id = public.id.clone();
    /// let id = select_image_from_sources(Vec::new(), vec![public], &request)?;
    /// assert_eq!(id, public_id);
    ///
    /// // A non-empty project list wins even when the public list would also
    /// // match. Distinct IDs show which source actually supplied the winner.
    /// let project_id = project.id.clone();
    /// let id = select_image_from_sources(vec![project], vec![unrelated], &request)?;
    /// assert_eq!(id, project_id);
    /// ```
    pub(in crate::scaleway) fn select_image_from_sources(
        project_images: Vec<ScalewayImage>,
        public_images: Vec<ScalewayImage>,
        request: &InstanceRequest,
    ) -> Result<ImageRoot, ScalewayBackendError> {
        let primary = if project_images.is_empty() {
            public_images
        } else {
            project_images
        };

        // Architecture and availability are unconditional, so they are applied
        // first; the storage filter then narrows to images that can back the
        // requested boot volume, before "newest wins" chooses among them. The
        // observed types are captured before the filter consumes the list.
        let usable = Self::filter_images(primary, request);
        let observed = Self::observed_root_types(&usable);
        let compatible = Self::filter_by_storage(usable, request.boot_volume.volume_type);

        if compatible.is_empty() && !observed.is_empty() {
            return Err(ScalewayBackendError::ImageStorageIncompatible {
                label: request.image_label.clone(),
                arch: request.architecture.clone(),
                zone: request.zone.clone(),
                observed,
                requested: request.boot_volume.volume_type,
            });
        }

        Self::select_image_id(compatible, request)
    }

    /// Keeps images whose architecture matches the request and whose state is
    /// `available`.
    ///
    /// For example, wrong-architecture and non-available images are dropped:
    ///
    /// ```ignore
    /// let kept = filter_images(
    ///     vec![
    ///         image("ok", "2026-01-01T00:00:00Z"),
    ///         with_state("starting", image("booting", "2026-01-01T00:00:00Z")),
    ///         with_arch("arm64", image("other-arch", "2026-01-01T00:00:00Z")),
    ///     ],
    ///     &request,
    /// );
    /// assert_eq!(kept.len(), 1);
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

    /// Keeps images whose own root volume can back `requested`.
    ///
    /// An unset `requested` keeps every image, because the type is then
    /// whatever the compatibility default resolves to.
    ///
    /// For example, asking for a local boot volume drops an SBS-only image
    /// even when that image is the newest.
    pub(in crate::scaleway) fn filter_by_storage(
        images: Vec<ScalewayImage>,
        requested: Option<BootVolumeType>,
    ) -> Vec<ScalewayImage> {
        let Some(wanted) = requested else {
            return images;
        };
        images
            .into_iter()
            .filter(|image| {
                image_storage_from_type(&image.root_volume.volume_type)
                    .is_ok_and(|storage| storage.supports(wanted))
            })
            .collect()
    }

    /// Lists the distinct root-volume types a candidate set reported.
    ///
    /// Sorted and deduplicated so the same candidate set always produces the
    /// same wording, whichever order the provider returned it in.
    ///
    /// For example, two images reporting `sbs_snapshot` yield
    /// `sbs_snapshot`, while a mixed pair yields `l_ssd, sbs_snapshot`.
    fn observed_root_types(images: &[ScalewayImage]) -> String {
        let mut observed: Vec<&str> = images
            .iter()
            .map(|image| image.root_volume.volume_type.as_str())
            .collect();
        observed.sort_unstable();
        observed.dedup();
        observed.join(", ")
    }
}
