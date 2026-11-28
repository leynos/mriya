//! Composition of an init request with the environment boundary.
//!
//! The `init` domain module must not depend on `ortho_config` or on the
//! process environment. This module is the adapter: it scans the injected
//! environment source, translates the result into the domain's
//! [`VolumeIdSource`], and builds the request the binary runs with.

use crate::{
    ScalewayConfig,
    init::{InitConfig, InitRequest, InitRequestError, VolumeIdSource},
};

/// Environment variable that supplies the volume ID directly.
pub const VOLUME_ID_ENV_VAR: &str = "SCW_DEFAULT_VOLUME_ID";

/// Reads the volume-ID variable through an injected environment boundary.
///
/// The key match is case-insensitive, because the loader's environment
/// provider matches keys that way; a guard that looked only for the canonical
/// spelling would miss a value the loader still applies.
///
/// # Parameters
///
/// - `env`: the scan source to read. It must describe the same environment that resolved the
///   configuration being checked; the loader's `process_scan_env_source` and a test `MapEnv` are
///   the expected sources.
///
/// # Returns
///
/// [`VolumeIdSource::None`] when no spelling of the variable is present,
/// [`VolumeIdSource::Environment`] with the trimmed value when it is usable,
/// and `Environment(String::new())` for a present blank value so the
/// orchestrator refuses it.
///
/// # Errors
///
/// Returns [`InitRequestError::NonUtf8EnvironmentVolumeId`] for a value the
/// boundary cannot decode.
pub fn environment_volume_id(
    env: &dyn ortho_config::ScanEnvSource,
) -> Result<VolumeIdSource, InitRequestError> {
    let mut found = VolumeIdSource::None;
    for (key, raw) in env.scan() {
        if !key
            .to_string_lossy()
            .eq_ignore_ascii_case(VOLUME_ID_ENV_VAR)
        {
            continue;
        }
        let text = raw
            .into_string()
            .map_err(|_| InitRequestError::NonUtf8EnvironmentVolumeId {
                variable: VOLUME_ID_ENV_VAR,
            })?;
        // A blank layer is still a layer: record it as an environment value so
        // the orchestrator refuses before any backend work.
        found = VolumeIdSource::Environment(text.trim().to_owned());
    }
    Ok(found)
}

/// The loaded configuration an init request is built from.
#[derive(Clone, Copy)]
pub struct InitRequestInputs<'a> {
    /// Scaleway configuration resolved from defaults, files, and environment.
    pub scaleway: &'a ScalewayConfig,
    /// Init-specific configuration.
    pub init_config: &'a InitConfig,
    /// Project name used for the volume name.
    pub project_name: &'a str,
    /// Whether a configured volume ID may be replaced.
    pub overwrite_existing_volume_id: bool,
}

/// Builds the init request the binary would use for a loaded configuration.
///
/// This is the composition root's whole decision: the request is built from
/// configuration, and the environment's own volume ID is translated into the
/// domain source so the orchestrator can refuse it.
///
/// # Parameters
///
/// - `inputs`: the loaded configuration and the overwrite flag. `scaleway` must have been loaded
///   from the same environment `env` describes.
/// - `env`: the scan source that detects the volume-ID variable, typically
///   `ortho_config::process_scan_env_source` in production and a `MapEnv` in tests.
///
/// # Returns
///
/// A request whose [`VolumeIdSource`] is `Environment` when the variable is
/// present (including blank) and otherwise the `File` or `None` source that
/// `InitRequest::from_config` recorded.
///
/// # Errors
///
/// Returns the configuration or environment errors described by
/// [`InitRequestError`].
pub fn request_from_loaded_config(
    inputs: InitRequestInputs<'_>,
    env: &dyn ortho_config::ScanEnvSource,
) -> Result<InitRequest, InitRequestError> {
    let source = environment_volume_id(env)?;
    let request = InitRequest::from_config(
        inputs.scaleway,
        inputs.init_config,
        inputs.project_name,
        inputs.overwrite_existing_volume_id,
    )?;
    Ok(request.with_volume_id_source(source))
}

#[cfg(test)]
mod tests {
    //! Unit tests for the environment-to-domain volume-ID translation.

    use std::ffi::OsStr;

    use ortho_config::MapEnv;

    use super::*;

    /// A scanning environment map with `vars` set.
    fn volume_env(vars: &[(&str, &OsStr)]) -> MapEnv {
        vars.iter()
            .fold(MapEnv::new(), |env, (key, value)| env.with_var(*key, value))
    }

    #[test]
    fn environment_volume_id_is_absent_when_unset() {
        let env = volume_env(&[]);

        let source = environment_volume_id(&env).expect("an unset variable should be accepted");

        assert_eq!(source, VolumeIdSource::None);
    }

    #[test]
    fn environment_volume_id_is_trimmed_when_set() {
        let env = volume_env(&[(VOLUME_ID_ENV_VAR, OsStr::new("  volume-123 "))]);

        let source = environment_volume_id(&env).expect("a set variable should be accepted");

        assert_eq!(
            source,
            VolumeIdSource::Environment(String::from("volume-123"))
        );
    }

    /// The loader matches environment keys case-insensitively, so the adapter
    /// must too.
    #[test]
    fn environment_volume_id_matches_a_case_variant_key() {
        let env = volume_env(&[("scw_default_volume_id", OsStr::new("old-id"))]);

        let source = environment_volume_id(&env).expect("a case variant should be accepted");

        assert_eq!(source, VolumeIdSource::Environment(String::from("old-id")));
    }

    #[test]
    fn environment_volume_id_records_a_blank_value() {
        let env = volume_env(&[(VOLUME_ID_ENV_VAR, OsStr::new("   "))]);

        let source = environment_volume_id(&env).expect("a blank value is recorded");

        assert_eq!(source, VolumeIdSource::Environment(String::new()));
    }

    #[cfg(unix)]
    #[test]
    fn environment_volume_id_rejects_a_non_utf8_value() {
        use std::os::unix::ffi::OsStrExt as _;

        let env = volume_env(&[(VOLUME_ID_ENV_VAR, OsStr::from_bytes(&[0xff]))]);

        let err = environment_volume_id(&env).expect_err("a non-UTF-8 value must be rejected");

        assert!(
            err.to_string().contains("not valid UTF-8"),
            "unexpected error: {err}"
        );
    }

    proptest::proptest! {
        /// Any casing of the canonical spelling is found and trimmed.
        #[test]
        fn environment_volume_id_matches_any_key_casing(
            bits in proptest::collection::vec(
                proptest::prelude::any::<bool>(),
                VOLUME_ID_ENV_VAR.len()..=VOLUME_ID_ENV_VAR.len(),
            ),
            id in "[A-Za-z0-9_-]{1,24}",
            leading in "[ \\t]{0,4}",
            trailing in "[ \\t]{0,4}",
        ) {
            let key: String = VOLUME_ID_ENV_VAR
                .chars()
                .zip(bits.iter())
                .map(|(character, uppercase)| {
                    if *uppercase {
                        character.to_ascii_uppercase()
                    } else {
                        character.to_ascii_lowercase()
                    }
                })
                .collect();
            let value = format!("{leading}{id}{trailing}");
            let env = MapEnv::new().with_var(key.as_str(), value.as_str());

            let source = environment_volume_id(&env).expect("a UTF-8 value should be accepted");

            assert_eq!(source, VolumeIdSource::Environment(id.clone()));
        }
    }
}
