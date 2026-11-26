//! The root volume an instance reports, and the allocation pairing it with the
//! plan that was requested.
//!
//! This is the "what was bought" half of the boot-volume model: the plan says
//! what the request asked for, and the allocation says what the account is now
//! paying for. Keeping them together lets a caller verify the two agree
//! without holding both separately.

use thiserror::Error;

use super::{BootVolumePlan, BootVolumeType, format_size};

/// The root volume an instance actually reports after creation.
///
/// This is the provider's answer, not the request: it is what the run
/// actually paid for, and what issue #154 needs in order to clean up.
///
/// # Examples
///
/// ```ignore
/// let root = RootVolume {
///     id: String::from("vol-root"),
///     volume_type: BootVolumeType::Local,
///     size_bytes: 20_000_000_000,
/// };
/// assert_eq!(root.volume_type.provider_id(), "l_ssd");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootVolume {
    /// Provider identifier of the root volume.
    pub id: String,
    /// Provider-reported storage type.
    pub volume_type: BootVolumeType,
    /// Provider-reported capacity in bytes.
    pub size_bytes: u64,
}

/// The boot volume a created instance ended up with.
///
/// The handle carries this so later stages can report and clean up the real
/// allocation rather than re-deriving it from the request.
///
/// # Examples
///
/// ```ignore
/// let allocation = BootVolumeAllocation {
///     plan: BootVolumePlan::new(BootVolumeType::Local, 20_000_000_000, Default::default()),
///     root: RootVolume { id: String::from("vol-root"), ..root },
/// };
/// assert_eq!(allocation.root.id, "vol-root");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootVolumeAllocation {
    /// The plan the create request asked for.
    pub plan: BootVolumePlan,
    /// The root volume the provider reported.
    pub root: RootVolume,
}

/// Errors raised when a reported root volume does not match the plan.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BootVolumeMismatch {
    /// Raised when the provider reports no bootable root volume at all.
    #[error("instance {instance_id} reported no bootable root volume")]
    Missing {
        /// Instance whose root volume was missing.
        instance_id: String,
    },
    /// Raised when the reported type or size differs from the plan.
    // thiserror 1.x accepts only a literal here, so the line is joined with a
    // continuation escape; `concat!` is rejected outright.
    #[error(
        "instance {instance_id} root volume {volume_id} is {actual}, but the plan \
         requested {expected}"
    )]
    Differs {
        /// Instance holding the root volume.
        instance_id: String,
        /// Provider identifier of the root volume.
        volume_id: String,
        /// What the plan asked for, already formatted.
        expected: String,
        /// What the provider reported, already formatted.
        actual: String,
    },
}

impl BootVolumeMismatch {
    /// Builds the mismatch for a reported root volume that differs from
    /// `plan`.
    ///
    /// For example, a local plan answered by an SBS root volume yields
    /// [`BootVolumeMismatch::Differs`] naming both.
    #[must_use]
    pub fn differs(instance_id: &str, root: &RootVolume, plan: &BootVolumePlan) -> Self {
        Self::Differs {
            instance_id: instance_id.to_owned(),
            volume_id: root.id.clone(),
            expected: format!(
                "{} {}",
                plan.volume_type.provider_id(),
                format_size(plan.size_bytes)
            ),
            actual: format!(
                "{} {}",
                root.volume_type.provider_id(),
                format_size(root.size_bytes)
            ),
        }
    }
}

impl BootVolumeAllocation {
    /// Verifies that `root` is the volume `plan` asked for.
    ///
    /// # Errors
    ///
    /// Returns [`BootVolumeMismatch::Differs`] when the type or size differs.
    ///
    /// For example, a plan for a 20 GB `l_ssd` accepts exactly that root and
    /// rejects a 21 GB one.
    pub fn verify(
        instance_id: &str,
        plan: &BootVolumePlan,
        root: RootVolume,
    ) -> Result<Self, BootVolumeMismatch> {
        if root.volume_type != plan.volume_type || root.size_bytes != plan.size_bytes {
            return Err(BootVolumeMismatch::differs(instance_id, &root, plan));
        }
        Ok(Self { plan: *plan, root })
    }
}
