//! Unit tests for boot-volume configuration and its validation.
//!
//! These live in their own test binary so the shared configuration fixtures
//! stay in `config_tests` and each file stays small. The settings are decimal
//! gigabytes and are kept separate from the cache volume, which is what the
//! cases below pin.

#[path = "common/test_constants.rs"]
mod test_constants;

use mockable::MockEnv;
use mriya::{BootVolumeType, ScalewayConfig};
use rstest::*;
use test_constants::DEFAULT_INSTANCE_TYPE;

/// An environment whose `HOME` is unset, so `as_request_with_env` reads no
/// process environment.
fn unset_home() -> MockEnv {
    let mut env = MockEnv::new();
    env.expect_raw()
        .returning(|_| Err(std::env::VarError::NotPresent));
    env.expect_string().returning(|_| None);
    env
}

/// A configuration with every required field present.
#[fixture]
fn valid_config() -> ScalewayConfig {
    ScalewayConfig {
        access_key: Some(String::from("SCWACCESSKEYEXAMPLE")),
        secret_key: String::from("SCWSECRETKEYEXAMPLE"),
        default_organization_id: None,
        default_project_id: String::from("11111111-2222-3333-4444-555555555555"),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from(DEFAULT_INSTANCE_TYPE),
        default_image: String::from("ubuntu-22-04"),
        default_architecture: String::from("x86_64"),
        default_volume_id: None,
        boot_volume_type: None,
        boot_volume_size_gb: None,
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    }
}

/// One boot-setting case: what the profile says and what the request should
/// carry.
#[derive(Clone, Copy, Debug)]
struct BootSettingCase {
    /// Configured `boot_volume_type`.
    configured_type: Option<&'static str>,
    /// Configured `boot_volume_size_gb`.
    configured_size: Option<u32>,
    /// Type the request should carry.
    expected_type: Option<BootVolumeType>,
    /// Size the request should carry.
    expected_size: Option<u32>,
}

/// Boot-volume settings reach the request unchanged, keeping the cache volume
/// separate.
///
/// The cache volume ID and the boot-storage settings travel on different
/// fields: a request that sets both must show both, and a request that sets
/// only one must leave the other at its default.
#[rstest]
#[case::both_explicit(BootSettingCase {
    configured_type: Some("l_ssd"),
    configured_size: Some(20),
    expected_type: Some(BootVolumeType::Local),
    expected_size: Some(20),
})]
#[case::type_only(BootSettingCase {
    configured_type: Some("sbs_volume"),
    configured_size: None,
    expected_type: Some(BootVolumeType::Sbs),
    expected_size: None,
})]
#[case::size_only(BootSettingCase {
    configured_type: None,
    configured_size: Some(30),
    expected_type: None,
    expected_size: Some(30),
})]
#[case::neither(BootSettingCase {
    configured_type: None,
    configured_size: None,
    expected_type: None,
    expected_size: None,
})]
fn boot_volume_settings_reach_the_request(
    valid_config: ScalewayConfig,
    #[case] case: BootSettingCase,
) {
    let cfg = ScalewayConfig {
        default_volume_id: Some(String::from("vol-cache")),
        boot_volume_type: case.configured_type.map(str::to_owned),
        boot_volume_size_gb: case.configured_size,
        ..valid_config
    };

    let request = cfg
        .as_request_with_env(&unset_home())
        .expect("boot-volume settings should produce a request");

    assert_eq!(request.boot_volume.volume_type, case.expected_type);
    assert_eq!(request.boot_volume.size_gb, case.expected_size);
    assert_eq!(
        request.volume_id.as_deref(),
        Some("vol-cache"),
        "the cache volume must be unaffected by the boot settings"
    );
}

/// A boot-volume type the provider no longer accepts is refused by name,
/// naming both routes a user can change.
#[rstest]
fn boot_volume_type_rejects_unsupported_identifiers(valid_config: ScalewayConfig) {
    let cfg = ScalewayConfig {
        boot_volume_type: Some(String::from("b_ssd")),
        ..valid_config
    };

    let error = cfg
        .resolve_boot_volume()
        .expect_err("a deprecated volume type must be refused");
    let message = error.to_string();
    assert!(message.contains("b_ssd"), "{message}");
    assert!(message.contains("SCW_BOOT_VOLUME_TYPE"), "{message}");
    assert!(message.contains("boot_volume_type"), "{message}");
}

/// A blank boot-volume type is treated as unset rather than refused, so an
/// empty environment variable cannot break a run.
#[rstest]
#[case::blank("   ")]
#[case::empty("")]
fn blank_boot_volume_type_is_unset(valid_config: ScalewayConfig, #[case] value: &str) {
    let cfg = ScalewayConfig {
        boot_volume_type: Some(value.to_owned()),
        ..valid_config
    };

    let resolved = cfg
        .resolve_boot_volume()
        .expect("a blank type should be treated as unset");
    assert_eq!(resolved.volume_type, None);
}

/// A zero boot-volume size is refused, naming the variable and the key.
#[rstest]
fn boot_volume_size_rejects_zero(valid_config: ScalewayConfig) {
    let cfg = ScalewayConfig {
        boot_volume_size_gb: Some(0),
        ..valid_config
    };

    let error = cfg
        .resolve_boot_volume()
        .expect_err("a zero size must be refused");
    let message = error.to_string();
    assert!(message.contains("SCW_BOOT_VOLUME_SIZE_GB"), "{message}");
    assert!(message.contains("boot_volume_size_gb"), "{message}");
}
