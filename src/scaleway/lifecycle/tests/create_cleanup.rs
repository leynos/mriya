//! Tests for destroying a server after a failed create.
//!
//! A create can fail after the POST: the root volume may disagree with the
//! plan, the cache may not attach, or the server may refuse to power on. Each
//! of those must still leave the account without a billing server, and the
//! original reason must survive the cleanup.
//!
//! `create` itself cannot be driven end-to-end here because image lookup goes
//! through the SDK against the production host, so these cases drive the
//! follow-up step directly.

use anyhow::Result;
use rstest::rstest;

use super::{
    backend_fixture,
    handle as existing_handle,
    http_stub::{Reply, only_request, request_url, with_stub},
    is_differing_mismatch,
    root_volume::local_server,
};
use crate::{
    boot_volume::{BootVolumePlan, BootVolumeType, CompatibilityDefault},
    scaleway::{
        ScalewayBackend,
        ScalewayBackendError,
        lifecycle::{CreateFailure, CreatedInstance},
    },
};

/// A minimal request, used by the cleanup cases.
fn request() -> crate::backend::InstanceRequest {
    crate::backend::InstanceRequest::builder()
        .image_label("image")
        .instance_type("type")
        .zone("zone")
        .project_id("proj")
        .architecture("x86_64")
        .build()
        .expect("a complete request should build")
}

/// A handle for the server the cleanup cases destroy.
fn handle() -> crate::backend::InstanceHandle {
    crate::backend::InstanceHandle {
        id: String::from("srv-1"),
        zone: String::from("zone"),
        boot_volume: None,
    }
}

/// A successful delete response.
fn deleted() -> Reply {
    Reply {
        status: 204,
        body: String::new(),
    }
}

/// Scenario: a post-POST failure destroys the server it created.
///
/// Invariant: the original mismatch is returned unchanged, and the DELETE the
/// cleanup issued reaches the substitutable API base, so a failed create does
/// not leave a billing instance behind.
#[rstest]
#[tokio::test]
async fn a_failed_create_destroys_the_server(backend_fixture: ScalewayBackend) -> Result<()> {
    let server = local_server()?;
    // A 10 GB plan against a 20 GB root fails verification.
    let plan = BootVolumePlan::new(
        BootVolumeType::Local,
        10_000_000_000,
        CompatibilityDefault::default(),
    );

    let (result, requests) = with_stub(backend_fixture, vec![deleted()], async |backend| {
        let request = request();
        let failure = backend
            .finish_created_server(CreatedInstance {
                request: &request,
                server: &server,
                plan: &plan,
                handle: handle(),
            })
            .await
            .expect_err("a mismatched root must fail the create");
        Ok::<_, ScalewayBackendError>(backend.clean_up_after_failed_create(failure).await)
    })
    .await?;

    // The cleanup reports the original failure as its success value, because
    // the work under test is "what does cleanup return", not "did it fail".
    let error = result.expect("the cleanup outcome should be reported");
    assert!(
        is_differing_mismatch(&error),
        "a successful cleanup must not change the error: {error:?}"
    );
    let request = super::http_stub::only_request(&requests)?;
    assert!(request.starts_with("DELETE "), "{request}");
    assert_eq!(
        request_url(request)?.path(),
        "/instance/v1/zones/zone/servers/srv-1"
    );
    Ok(())
}

/// Scenario: the cleanup delete is itself rejected.
///
/// Invariant: the original error survives as the source while the message
/// carries the teardown outcome, so an operator can see both that the create
/// failed and that the server may still exist.
#[rstest]
#[tokio::test]
async fn a_failed_cleanup_reports_both_outcomes(backend_fixture: ScalewayBackend) -> Result<()> {
    let server = local_server()?;
    let plan = BootVolumePlan::new(
        BootVolumeType::Local,
        10_000_000_000,
        CompatibilityDefault::default(),
    );
    let rejection = Reply {
        status: 500,
        body: String::from(r#"{"message":"boom"}"#),
    };

    let (result, _) = with_stub(backend_fixture, vec![rejection], async |backend| {
        let request = request();
        let failure = backend
            .finish_created_server(CreatedInstance {
                request: &request,
                server: &server,
                plan: &plan,
                handle: handle(),
            })
            .await
            .expect_err("a mismatched root must fail the create");
        Ok::<_, ScalewayBackendError>(backend.clean_up_after_failed_create(failure).await)
    })
    .await?;

    let error = result.expect("the cleanup outcome should be reported");
    let ScalewayBackendError::CreateCleanupFailed { message, source } = &error else {
        panic!("expected a combined cleanup failure, got {error:?}");
    };
    assert!(message.contains("teardown also failed"), "{message}");
    assert!(
        is_differing_mismatch(source),
        "the original failure must be preserved as the source: {source:?}"
    );
    Ok(())
}

/// Scenario: an attachment failure whose outcome is unknown.
///
/// Invariant: a transport failure is settled by reading the instance back. A
/// cache the provider reports as attached keeps the server, one it reports as
/// absent allows the delete, and an unreadable instance keeps the server
/// because detachment is unproven.
#[rstest]
#[case::attached(attached_instance(), true)]
#[case::absent(instance_without_cache(), false)]
#[case::unreadable(rejected(), true)]
#[tokio::test]
async fn an_ambiguous_attachment_failure_is_settled_by_a_read(
    backend_fixture: ScalewayBackend,
    #[case] reply: Reply,
    #[case] expect_keep: bool,
) -> Result<()> {
    let handle = existing_handle();
    let (result, requests) = with_stub(backend_fixture, vec![reply], async |backend| {
        Ok::<_, crate::scaleway::ScalewayBackendError>(
            backend
                .classify_attachment_failure(
                    &handle,
                    "vol-cache",
                    ScalewayBackendError::Provider {
                        message: String::from("connection reset"),
                    },
                )
                .await,
        )
    })
    .await?;

    let failure = result.expect("classification should be reported");
    assert_eq!(failure.keep_server, expect_keep, "{failure:?}");
    assert_eq!(requests.len(), 1, "exactly one read should be made");
    assert!(only_request(&requests)?.starts_with("GET "));
    Ok(())
}

/// Scenario: the provider rejected the attachment and the instance confirms
/// nothing was attached.
///
/// Invariant: the read settles every attachment failure, whatever the error
/// kind, and an instance without the cache allows the delete.
#[rstest]
#[tokio::test]
async fn a_rejected_attachment_allows_cleanup(backend_fixture: ScalewayBackend) -> Result<()> {
    let handle = existing_handle();
    let (result, requests) = with_stub(
        backend_fixture,
        vec![instance_without_cache()],
        async |backend| {
            Ok::<_, crate::scaleway::ScalewayBackendError>(
                backend
                    .classify_attachment_failure(
                        &handle,
                        "vol-cache",
                        ScalewayBackendError::VolumeAttachmentFailed {
                            volume_id: String::from("vol-cache"),
                            instance_id: String::from("id"),
                            message: String::from("conflict"),
                        },
                    )
                    .await,
            )
        },
    )
    .await?;

    let failure = result.expect("classification should be reported");
    assert!(!failure.keep_server, "{failure:?}");
    assert_eq!(requests.len(), 1, "the settlement read should run");
    assert!(only_request(&requests)?.starts_with("GET "));
    Ok(())
}

/// Scenario: a retained server must not be deleted by the cleanup step.
///
/// Invariant: `clean_up_after_failed_create` honours the retention decision and
/// issues no DELETE, even though it is the same call the ordinary failure path
/// makes.
#[rstest]
#[tokio::test]
async fn cleanup_keeps_a_server_marked_for_retention(
    backend_fixture: ScalewayBackend,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![], async |backend| {
        let failure = Box::new(CreateFailure {
            error: ScalewayBackendError::Provider {
                message: String::from("power on failed"),
            },
            handle: handle(),
            keep_server: true,
        });
        Ok::<_, crate::scaleway::ScalewayBackendError>(
            backend.clean_up_after_failed_create(failure).await,
        )
    })
    .await?;

    let error = result.expect("the combined failure should be reported");
    let ScalewayBackendError::CreateCleanupFailed { message, .. } = &error else {
        panic!("expected a cleanup failure, got {error:?}");
    };
    assert!(message.contains("left running"), "{message}");
    assert!(
        requests.is_empty(),
        "a retained server must not be deleted: {requests:?}"
    );
    Ok(())
}

/// A provider response denying the instance read.
fn rejected() -> Reply {
    Reply {
        status: 404,
        body: String::from(r#"{"type":"denied","message":"no such instance"}"#),
    }
}

/// A provider response listing an instance with the cache volume attached.
fn attached_instance() -> Reply {
    Reply {
        status: 200,
        body: include_str!("fixtures/detach-instance.json").to_owned(),
    }
}

/// A provider response listing an instance with only its root volume.
fn instance_without_cache() -> Reply {
    let mut instance: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/detach-instance.json"))
            .expect("detach fixture is JSON");
    instance
        .pointer_mut("/server/volumes")
        .and_then(serde_json::Value::as_object_mut)
        .expect("fixture volumes")
        .remove("1");
    Reply {
        status: 200,
        body: serde_json::to_string(&instance).expect("serialize fixture"),
    }
}
