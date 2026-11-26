//! Scripted failure modes and state shared by the init backend double.

use mriya::{VolumeState, VolumeStatus};

#[derive(Clone, Copy, Debug)]
pub(super) enum FailureMode {
    CreateVolume,
    VolumeReady,
    Provision,
    Wait,
    Detach,
    Destroy,
    DeleteVolume,
}

impl FailureMode {
    pub(super) const fn flag(self) -> u8 {
        match self {
            Self::CreateVolume => 0b0000_0001,
            Self::VolumeReady => 0b0000_0010,
            Self::Provision => 0b0000_0100,
            Self::Wait => 0b0000_1000,
            Self::Detach => 0b0001_0000,
            Self::Destroy => 0b0010_0000,
            Self::DeleteVolume => 0b0100_0000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Failures(u8);

impl Failures {
    pub(super) const fn set(&mut self, mode: FailureMode) { self.0 |= mode.flag(); }

    pub(super) const fn contains(self, mode: FailureMode) -> bool { self.0 & mode.flag() != 0 }
}

/// Lifecycle event recorded by the init backend double, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LifecycleEvent {
    /// The new volume reached `available`.
    VolumeReady,
    /// A formatter server was created, which attaches the cache.
    InstanceCreated,
    /// The formatter was torn down through the ordered teardown.
    Teardown,
    /// The owned volume was deleted.
    VolumeDeleted,
}

/// Builds the available volume state the doubles report.
pub(super) fn volume_state() -> VolumeState {
    VolumeState {
        id: String::from("vol-123"),
        zone: String::from("test-zone"),
        project_id: String::from("project"),
        status: VolumeStatus::Available,
        volume_type: String::from("sbs_5k"),
        storage_class: Some(String::from("sbs")),
        size_bytes: 10_000_000_000,
        references: Vec::new(),
    }
}
