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
    http_stub::{Reply, request_url, with_stub},
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

/// Each attachment status and whether the server may be destroyed.
///
/// A client error proves the PATCH did not apply. A server error or a transport
/// failure leaves the outcome unknown, so the server is kept: a timed-out
/// request can commit after any read-back, and destroying the server could take
/// the cache with it.
#[rstest]
#[case::bad_request(400, false)]
#[case::conflict(409, false)]
#[case::unprocessable(422, false)]
#[case::server_error(500, true)]
#[case::bad_gateway(502, true)]
#[case::unavailable(503, true)]
#[tokio::test]
async fn attachment_failures_decide_whether_the_server_is_kept(
    backend_fixture: ScalewayBackend,
    #[case] status: u16,
    #[case] expect_keep: bool,
) -> Result<()> {
    let handle = existing_handle();
    let (result, requests) = with_stub(backend_fixture, vec![], async |_backend| {
        Ok::<_, crate::scaleway::ScalewayBackendError>(
            ScalewayBackend::classify_attachment_failure(
                &handle,
                "vol-cache",
                ScalewayBackendError::VolumeAttachmentFailed {
                    volume_id: String::from("vol-cache"),
                    instance_id: String::from("id"),
                    status,
                    message: String::from("conflict"),
                },
            ),
        )
    })
    .await?;

    let failure = result.expect("classification should be reported");
    assert_eq!(failure.keep_server, expect_keep, "{failure:?}");
    assert!(
        requests.is_empty(),
        "classification makes no request: {requests:?}"
    );
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
