//! Unit tests for Scaleway backend tagging, error classification, and
//! volume validation.
//!
//! Kills the error-classification and cache-volume validation survivors
//! tracked in #57.
use rstest::rstest;

use super::ScalewayBackend;
use crate::{
    backend::{InstanceHandle, InstanceRequest},
    scaleway::ScalewayBackendError,
};

/// Builds a provider error with the supplied classification fields.
fn api_error(
    etype: &str,
    resource: Option<&str>,
    resource_id: Option<&str>,
) -> scaleway_rs::ScalewayApiError {
    scaleway_rs::ScalewayApiError {
        message: String::from("boom"),
        resource: resource.map(str::to_owned),
        resource_id: resource_id.map(str::to_owned),
        etype: etype.to_owned(),
    }
}

/// Creates a request whose instance type can be varied by a test case.
fn request_with_type(instance_type: &str) -> InstanceRequest {
    InstanceRequest {
        image_label: String::from("label"),
        instance_type: instance_type.to_owned(),
        zone: String::from("zone"),
        project_id: String::from("proj"),
        organisation_id: None,
        architecture: String::from("x86_64"),
        volume_id: None,
        cloud_init_user_data: None,
    }
}

/// Supplies the stable instance identity used by volume error assertions.
fn handle() -> InstanceHandle {
    InstanceHandle {
        id: String::from("instance-1"),
        zone: String::from("zone"),
    }
}

#[derive(Clone, Copy)]
struct CacheVolumeCase {
    cache_volume_id: &'static str,
    root_volume_id: &'static str,
    expect_rejection: bool,
}

type TagBuilder = fn(Option<&str>) -> Vec<String>;

#[derive(Clone, Copy)]
struct TagCase {
    builder: TagBuilder,
    base_tag: &'static str,
    test_run_id: Option<&'static str>,
    expected_test_tag: Option<&'static str>,
}

/// Covers both type-identifying fields and unrelated provider errors.
#[rstest]
// Resource alone identifies the instance type, without a resource_id.
#[case(api_error("invalid_arguments", Some("commercial_type"), None), true)]
// A resource_id matching the requested type classifies even when the
// resource is unrelated.
#[case(api_error("unknown", Some("server"), Some("DEV1-S")), true)]
// A non-matching resource_id must not classify.
#[case(api_error("unknown", Some("server"), Some("GP1-XS")), false)]
// An unrelated error must not classify.
#[case(api_error("out_of_stock", None, None), false)]
fn is_instance_type_error_classifies(
    #[case] api_err: scaleway_rs::ScalewayApiError,
    #[case] expected: bool,
) {
    let request = request_with_type("DEV1-S");
    assert_eq!(
        ScalewayBackend::is_instance_type_error(&api_err, &request),
        expected,
        "unexpected classification for {api_err:?}"
    );
}

/// Rejects equal trimmed volume IDs while accepting distinct IDs.
#[rstest]
#[case::same_trimmed_id(CacheVolumeCase {
    cache_volume_id: " vol-1 ", root_volume_id: "vol-1", expect_rejection: true,
})]
#[case::distinct_id(CacheVolumeCase {
    cache_volume_id: "vol-1", root_volume_id: "vol-2", expect_rejection: false,
})]
fn validate_cache_volume_id_rejects_only_the_root_volume(#[case] case: CacheVolumeCase) {
    let result = ScalewayBackend::validate_cache_volume_id(
        case.cache_volume_id,
        case.root_volume_id,
        &handle(),
    );
    let rejected = matches!(
        result,
        Err(ScalewayBackendError::VolumeAttachmentFailed { .. })
    );
    assert_eq!(
        rejected, case.expect_rejection,
        "cache={:?} root={:?}: {result:?}",
        case.cache_volume_id, case.root_volume_id
    );
}

/// Includes test-run tags only for nonblank, trimmed identifiers.
#[rstest]
#[case::instance_absent(TagCase {
    builder: ScalewayBackend::instance_tags, base_tag: "ephemeral",
    test_run_id: None, expected_test_tag: None,
})]
#[case::volume_absent(TagCase {
    builder: ScalewayBackend::volume_tags, base_tag: "cache",
    test_run_id: None, expected_test_tag: None,
})]
#[case::instance_blank(TagCase {
    builder: ScalewayBackend::instance_tags, base_tag: "ephemeral",
    test_run_id: Some("   "), expected_test_tag: None,
})]
#[case::volume_blank(TagCase {
    builder: ScalewayBackend::volume_tags, base_tag: "cache",
    test_run_id: Some("\t\n"), expected_test_tag: None,
})]
#[case::instance_nonblank(TagCase {
    builder: ScalewayBackend::instance_tags, base_tag: "ephemeral",
    test_run_id: Some("run-123"), expected_test_tag: Some("mriya-test-run-run-123"),
})]
#[case::volume_nonblank(TagCase {
    builder: ScalewayBackend::volume_tags, base_tag: "cache",
    test_run_id: Some("run-123"), expected_test_tag: Some("mriya-test-run-run-123"),
})]
#[case::instance_padded(TagCase {
    builder: ScalewayBackend::instance_tags, base_tag: "ephemeral",
    test_run_id: Some("  run-123  "), expected_test_tag: Some("mriya-test-run-run-123"),
})]
#[case::volume_padded(TagCase {
    builder: ScalewayBackend::volume_tags, base_tag: "cache",
    test_run_id: Some("\t run-123\n"), expected_test_tag: Some("mriya-test-run-run-123"),
})]
fn tags_include_test_run_tag_only_for_nonblank_id(#[case] case: TagCase) {
    let mut expected_tags = vec![String::from("mriya"), String::from(case.base_tag)];
    if let Some(test_tag) = case.expected_test_tag {
        expected_tags.push(String::from(test_tag));
    }
    assert_eq!(
        (case.builder)(case.test_run_id),
        expected_tags,
        "base={:?} id={:?}",
        case.base_tag,
        case.test_run_id
    );
}

fn backend_config() -> crate::ScalewayConfig {
    crate::ScalewayConfig {
        access_key: None,
        secret_key: String::from("secret"),
        default_organization_id: None,
        default_project_id: String::from("11111111-2222-3333-4444-555555555555"),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from("DEV1-S"),
        default_image: String::from("ubuntu-22-04"),
        default_architecture: String::from("x86_64"),
        default_volume_id: None,
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    }
}

#[rstest]
#[case::unset(&[], None)]
#[case::set(&[(crate::TEST_RUN_ID_ENV, "run-9")], Some("run-9"))]
fn new_with_env_reads_test_run_id_from_injected_environment(
    #[case] vars: &[(&str, &str)],
    #[case] expected: Option<&str>,
) {
    let env = crate::test_helpers::mock_env(vars);
    let backend = ScalewayBackend::new_with_env(backend_config(), &env)
        .expect("valid configuration should build a backend");
    assert_eq!(backend.test_run_id.as_deref(), expected);
}
