//! Supporting state for ownership-aware cleanup.
//!
//! These types keep the cleanup steps readable: one accumulator collects what
//! was verified and what remains, one set describes the volumes that must be
//! detached before a server can be deleted, and one enum reduces a volume to
//! the states cleanup decides on.

use crate::{
    cleanup::{CleanupFailure, CleanupReport},
    ownership::{Disposition, ResourceKind, ResourceLedger, ResourceRef},
    scaleway::lifecycle::{
        models::{ServerModel, VolumeModel},
        records::disposition_of,
    },
};

/// How the detachment of preserved volumes ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DetachOutcome {
    /// Every preserved volume was detached and verified.
    Detached,
    /// The server disappeared before the patch, so none remain attached.
    ServerGone,
    /// At least one preserved volume may still be attached.
    Failed,
}

/// State of one recorded volume, reduced to what cleanup decides on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum VolumeState {
    /// The volume still exists and is attached to the named server.
    AttachedTo(String),
    /// The volume exists and is not attached to any server.
    Detached,
    /// A successful listing confirmed the volume is gone.
    Absent,
}

/// Collects cleanup's verified removals and its unverified residue.
#[derive(Debug)]
pub(super) struct CleanupAccumulator {
    /// Successfully verified outcome.
    pub(super) report: CleanupReport,
    /// Resources and causes that could not be verified.
    pub(super) failure: CleanupFailure,
    /// Project the ledger is scoped to.
    pub(super) project_id: String,
}

impl CleanupAccumulator {
    /// Creates an accumulator for `project_id`.
    pub(super) fn new(project_id: String) -> Self {
        Self {
            report: CleanupReport::default(),
            failure: CleanupFailure::new(),
            project_id,
        }
    }

    /// Records a verified removal.
    pub(super) fn removed(&mut self, resource: ResourceRef) { self.report.removed.push(resource); }

    /// Records a verified preserved resource.
    pub(super) fn preserved(&mut self, resource: ResourceRef) {
        self.report.preserved.push(resource);
    }

    /// Records a residual resource with its cause.
    pub(super) fn add(&mut self, resource: ResourceRef, cause: impl Into<String>) {
        self.failure.add(resource, cause);
    }

    /// Records a failure that is not attached to one resource.
    pub(super) fn add_cause(&mut self, cause: impl Into<String>) { self.failure.add_cause(cause); }
}

/// The volumes to detach before deleting a server, and where they live.
pub(super) struct DetachSet<'a> {
    /// The server that holds them.
    server: &'a ServerModel,
    /// Zone the server lives in.
    pub(super) zone: &'a str,
    /// Identifiers of the volumes to detach.
    pub(super) volume_ids: Vec<String>,
}

impl<'a> DetachSet<'a> {
    /// Builds the set of volumes that must survive the server.
    pub(super) fn for_server(
        server: &'a ServerModel,
        ledger: &ResourceLedger,
        zone: &'a str,
    ) -> Self {
        Self {
            server,
            zone,
            volume_ids: volumes_to_preserve(server, ledger, zone),
        }
    }

    /// Returns the attached volumes named by this set.
    pub(super) fn attached_volumes(&self) -> impl Iterator<Item = &'a VolumeModel> + '_ {
        self.server
            .volumes
            .values()
            .filter(|volume| self.volume_ids.iter().any(|id| id == &volume.id))
    }
}

/// Returns the identifiers of the volumes the run must not delete.
///
/// Owned volumes are transient and are deleted after the server; everything
/// else — retained caches and attachments the ledger never recorded — is
/// detached first and preserved.
fn volumes_to_preserve(server: &ServerModel, ledger: &ResourceLedger, zone: &str) -> Vec<String> {
    server
        .volumes
        .values()
        .filter(|volume| {
            !matches!(
                disposition_of(ledger, volume, zone),
                Some(Disposition::Owned)
            )
        })
        .map(|volume| volume.id.clone())
        .collect()
}

/// Returns the identifiers of the owned resources of `kind` in `ledger`.
pub(super) fn owned_ids(ledger: &ResourceLedger, kind: ResourceKind) -> Vec<ResourceRef> {
    ledger
        .entries_with(Disposition::Owned)
        .into_iter()
        .filter(|entry| entry.resource.kind() == kind)
        .map(|entry| entry.resource)
        .collect()
}
