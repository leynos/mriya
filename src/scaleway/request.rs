//! Request construction and resource tagging for the Scaleway backend.

use super::{ScalewayBackend, ScalewayBackendError};
use crate::{
    backend::{InstanceHandle, InstanceRequest},
    janitor::TEST_RUN_TAG_PREFIX,
};

impl ScalewayBackend {
    /// Builds an instance request using the backend's defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when configuration validation
    /// fails.
    pub fn default_request(&self) -> Result<InstanceRequest, ScalewayBackendError> {
        self.default_request_with_env(&mockable::DefaultEnv)
    }

    /// Builds an instance request using the backend's defaults, reading `HOME`
    /// for a `~/` cloud-init file path through `env`.
    ///
    /// This is [`ScalewayBackend::default_request`] with an injectable
    /// environment, so a caller or test can resolve a tilde path without
    /// reading the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ScalewayBackendError::Config`] when configuration validation
    /// fails.
    pub fn default_request_with_env(
        &self,
        env: &impl mockable::Env,
    ) -> Result<InstanceRequest, ScalewayBackendError> {
        self.config
            .as_request_with_env(env)
            .map_err(ScalewayBackendError::from)
    }

    /// Rejects a cache volume that is the instance's root volume.
    ///
    /// IDs are compared after trimming whitespace. Attaching the root volume
    /// again as the cache volume would corrupt the volume map, so the request
    /// fails with `VolumeAttachmentFailed` instead.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Padding around the identifier is ignored when comparing.
    /// let err =
    ///     ScalewayBackend::validate_cache_volume_id(" vol-1 ", "vol-1", &handle).unwrap_err();
    /// assert!(matches!(err, ScalewayBackendError::VolumeAttachmentFailed { .. }));
    ///
    /// // Two different volumes pass, so the PATCH may proceed.
    /// ScalewayBackend::validate_cache_volume_id("vol-cache", "vol-1", &handle)?;
    /// ```
    pub(in crate::scaleway) fn validate_cache_volume_id(
        cache_volume_id: &str,
        root_volume_id: &str,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        if cache_volume_id.trim() == root_volume_id.trim() {
            return Err(ScalewayBackendError::VolumeAttachmentFailed {
                volume_id: cache_volume_id.trim().to_owned(),
                instance_id: handle.id.clone(),
                message: String::from("refuse to attach root volume as cache volume"),
            });
        }

        Ok(())
    }

    /// Appends the test-run tag and allocation tag, in that order.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// assert_eq!(
    ///     ScalewayBackend::build_tags(vec![String::from("mriya")], Some(" abc "), None),
    ///     vec![String::from("mriya"), format!("{TEST_RUN_TAG_PREFIX}abc")]
    /// );
    /// ```
    pub(in crate::scaleway) fn build_tags(
        base_tags: Vec<String>,
        test_run_id: Option<&str>,
        allocation: Option<&str>,
    ) -> Vec<String> {
        let mut tags = base_tags;
        if let Some(tag) = allocation.map(str::trim).filter(|tag| !tag.is_empty()) {
            tags.push(tag.to_owned());
        }
        if let Some(id) = test_run_id.map(str::trim).filter(|id| !id.is_empty()) {
            tags.push(format!("{TEST_RUN_TAG_PREFIX}{id}"));
        }
        tags
    }

    /// Tags applied to created instances: `mriya`, `ephemeral`, the
    /// allocation tag, and the optional test-run tag.
    pub(in crate::scaleway) fn instance_tags(
        test_run_id: Option<&str>,
        allocation: Option<&str>,
    ) -> Vec<String> {
        Self::build_tags(
            vec![String::from("mriya"), String::from("ephemeral")],
            test_run_id,
            allocation,
        )
    }

    /// Tags applied to created cache volumes: `mriya`, `cache`, the
    /// allocation tag, and the optional test-run tag.
    pub(in crate::scaleway) fn volume_tags(
        test_run_id: Option<&str>,
        allocation: Option<&str>,
    ) -> Vec<String> {
        Self::build_tags(
            vec![String::from("mriya"), String::from("cache")],
            test_run_id,
            allocation,
        )
    }
}
