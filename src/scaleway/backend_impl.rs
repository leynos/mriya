//! The Scaleway adapter's implementation of the `Backend` and `VolumeBackend`
//! ports.
//!
//! Kept apart from the adapter's construction and request-building helpers so
//! the port implementation reads as one lifecycle: create, wait, destroy, plus
//! the two volume operations. Observation events are emitted around the calls
//! that have provider consequences.

use super::{ScalewayBackend, ScalewayBackendError};
use crate::{
    backend::{Backend, BackendFuture, InstanceHandle, InstanceNetworking, InstanceRequest},
    volume::{VolumeBackend, VolumeHandle, VolumeRequest},
};

impl Backend for ScalewayBackend {
    type Error = ScalewayBackendError;

    fn create<'a>(
        &'a self,
        request: &'a InstanceRequest,
    ) -> BackendFuture<'a, InstanceHandle, Self::Error> {
        Box::pin(async move {
            request.validate()?;
            let image_id = self.resolve_image_id(request).await?;

            let server = self
                .create_instance_stopped_observed(request, &image_id)
                .await?;

            let handle = InstanceHandle {
                id: server.id.clone(),
                zone: request.zone.clone(),
            };

            // A server that exists but cannot be finished is still a resource
            // that will bill. Report the failure as an uncertain allocation so
            // the benchmark records an identifier to look for, rather than
            // reporting that nothing was created.
            match self.finish_creation(request, &server, &handle).await {
                Ok(()) => Ok(handle),
                Err(err) => Err(self.abandon_creation(&handle, err)),
            }
        })
    }

    fn wait_for_ready<'a>(
        &'a self,
        handle: &'a InstanceHandle,
    ) -> BackendFuture<'a, InstanceNetworking, Self::Error> {
        Box::pin(async move {
            let networking = self.wait_for_public_ip(handle).await?;
            self.wait_for_ssh_ready(handle, &networking).await?;
            Ok(networking)
        })
    }

    fn destroy(&self, handle: InstanceHandle) -> BackendFuture<'_, (), Self::Error> {
        Box::pin(async move {
            use crate::bench::lifecycle::LifecycleEvent;

            self.notify_lifecycle(&LifecycleEvent::TeardownRequested {
                provider: String::from(PROVIDER_NAME),
                resource_id: handle.id.clone(),
            });
            if let Err(err) = self.delete_instance(&handle).await {
                self.notify_lifecycle(&LifecycleEvent::TeardownUnverified {
                    provider: String::from(PROVIDER_NAME),
                    resource_id: handle.id.clone(),
                    reason: err.to_string(),
                });
                return Err(err);
            }
            match self.wait_until_gone(&handle).await {
                Ok(()) => {
                    self.notify_lifecycle(&LifecycleEvent::TeardownVerified {
                        provider: String::from(PROVIDER_NAME),
                        resource_id: handle.id.clone(),
                    });
                    Ok(())
                }
                Err(err) => {
                    self.notify_lifecycle(&LifecycleEvent::TeardownUnverified {
                        provider: String::from(PROVIDER_NAME),
                        resource_id: handle.id.clone(),
                        reason: err.to_string(),
                    });
                    Err(err)
                }
            }
        })
    }
}

impl VolumeBackend for ScalewayBackend {
    fn create_volume<'a>(
        &'a self,
        request: &'a VolumeRequest,
    ) -> BackendFuture<'a, VolumeHandle, Self::Error> {
        Box::pin(async move { Self::create_volume(self, request).await })
    }

    fn detach_volume<'a>(
        &'a self,
        handle: &'a InstanceHandle,
        volume_id: &'a str,
    ) -> BackendFuture<'a, (), Self::Error> {
        Box::pin(async move { Self::detach_volume(self, handle, volume_id).await })
    }
}

/// Provider name every Scaleway lifecycle event carries.
pub(super) const PROVIDER_NAME: &str = "scaleway";

/// Returns whether a create failure might still have created a resource.
///
/// Only an uncertain `Provider` failure qualifies: a transport, response-read,
/// or response-parse failure may have reached the provider. A validation
/// failure, a definite rejection, an unavailable instance type, or an image
/// lookup failure cannot have created anything.
pub(super) const fn may_have_created(err: &ScalewayBackendError) -> bool {
    matches!(err, ScalewayBackendError::Provider { .. })
}
