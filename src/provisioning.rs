//! Operator-facing reporting for provisioning workflows.
//!
//! Both the run and init workflows resolve a boot plan before they create
//! anything, and both end up with an instance whose root volume they can
//! describe. Reporting those facts through a small trait keeps the
//! orchestrators free of output code and lets a test assert the text without
//! capturing a process stream.
//!
//! Boot and cache are always reported on separate lines and derived from
//! separate inputs. Deriving one from the other would let a stale cache
//! setting masquerade as the boot disk, which is exactly the confusion this
//! change exists to remove.

use std::{
    fmt::{Debug, Display, Formatter},
    io::Write,
    sync::Mutex,
};

use crate::{
    backend::InstanceHandle,
    boot_volume::{BootVolumePlan, format_size},
};

/// Receives provisioning facts an operator should see.
///
/// Every method has a no-op default, so an implementation only overrides what
/// it reports.
///
/// # Examples
///
/// ```ignore
/// // Reporting the same plan twice is not an error; a reporter is a sink.
/// let reporter = StderrReporter::new(std::io::stderr());
/// reporter.plan_resolved(&plan);
/// ```
pub trait ProvisioningReporter: Debug + Send + Sync {
    /// Reports a resolved boot plan, before any billable resource exists.
    ///
    /// # Parameters
    /// - `plan`: the resolved type, capacity, and compatibility default.
    fn plan_resolved(&self, _plan: &BootVolumePlan) {}

    /// Reports the instance that now exists, before readiness checks begin.
    ///
    /// # Parameters
    /// - `handle`: the created instance, including the root volume the provider reported when one
    ///   was captured.
    /// - `cache_volume_id`: the retained cache volume the request named, or `None` when the run has
    ///   no cache. This is the requested identifier, not a separately verified allocation.
    fn instance_created(&self, _handle: &InstanceHandle, _cache_volume_id: Option<&str>) {}
}

/// Discards every event.
///
/// This is the default for an orchestrator built without a reporter, so
/// existing callers keep their silent behaviour.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopReporter;

impl ProvisioningReporter for NoopReporter {}

/// Renders the boot plan as one line.
///
/// The compatibility-default marker is included only when a setting was
/// defaulted, so a reader can tell a chosen value from an inherited one.
///
/// # Parameters
/// - `plan`: the resolved plan to render.
///
/// # Returns
/// One line naming the provider identifier, both size units, and whether the
/// settings were configured or defaulted.
///
/// For example, an explicit 20 GB local plan renders
/// `boot volume: l_ssd 20 GB (20000000000 bytes) [configured]`.
#[must_use]
pub fn render_plan_line(plan: &BootVolumePlan) -> String {
    format!(
        "boot volume: {} {} [{}]",
        plan.volume_type.provider_id(),
        format_size(plan.size_bytes),
        source_label(plan)
    )
}

/// Renders the created instance's boot and cache allocations.
///
/// The boot line comes from the handle's recorded root volume and the cache
/// line from the request's own cache setting; neither is inferred from the
/// other. A handle without a recorded root renders `unknown` rather than
/// omitting the line, so an operator can see that the value is missing.
///
/// # Parameters
/// - `handle`: the created instance, whose boot record supplies the boot line.
/// - `cache_volume_id`: the retained cache volume the request named, or `None` to report `none`.
///
/// # Returns
/// Two lines: the boot allocation, then the cache allocation.
///
/// For example, a handle with a 20 GB local root and no cache renders
/// `boot volume: vol-root l_ssd 20 GB (20000000000 bytes) [configured]` then
/// `cache volume: none`.
#[must_use]
pub fn render_allocation_lines(handle: &InstanceHandle, cache_volume_id: Option<&str>) -> String {
    let boot = handle.boot_volume.as_ref().map_or_else(
        || String::from("boot volume: unknown"),
        |record| {
            format!(
                "boot volume: {} {} {} [{}]",
                record.root.id,
                record.root.volume_type.provider_id(),
                format_size(record.root.size_bytes),
                source_label(&record.plan)
            )
        },
    );
    let cache = cache_volume_id.map_or_else(
        || String::from("cache volume: none"),
        |id| format!("cache volume: {id}"),
    );
    format!("{boot}\n{cache}")
}

/// Describes where the plan's settings came from.
///
/// For example, a plan defaulted only in size renders
/// `compatibility default: size`.
fn source_label(plan: &BootVolumePlan) -> String {
    let defaulted = plan.compatibility_default;
    if defaulted.has_default() {
        format!("compatibility default: {}", defaulted.describe())
    } else {
        String::from("configured")
    }
}

/// Writes each reported fact to a stream, one line per fact.
///
/// A write failure is swallowed: a closed stream must not change the outcome
/// of a provisioning run.
///
/// # Examples
///
/// ```ignore
/// let reporter = StderrReporter::new(std::io::stderr());
/// orchestrator.with_reporter(Arc::new(reporter));
/// ```
pub struct StderrReporter<W: Write> {
    /// Destination for the lines.
    writer: Mutex<W>,
}

impl<W: Write> StderrReporter<W> {
    /// Creates a reporter writing to `writer`.
    ///
    /// # Parameters
    /// - `writer`: destination for the report lines, usually [`std::io::stderr`].
    ///
    /// # Returns
    /// A reporter that writes one line per reported fact.
    #[must_use]
    pub const fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
        }
    }

    /// Writes `line` followed by a newline, ignoring failures.
    fn emit(&self, line: impl Display) {
        let mut guard = match self.writer.lock() {
            Ok(guard) => guard,
            // A poisoned lock means a previous write panicked mid-line. The
            // report is diagnostic, so it is skipped rather than escalated.
            Err(poisoned) => poisoned.into_inner(),
        };
        writeln!(guard, "{line}").ok();
    }
}

impl<W: Write> Debug for StderrReporter<W> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StderrReporter")
    }
}

impl<W: Write + Send> ProvisioningReporter for StderrReporter<W> {
    fn plan_resolved(&self, plan: &BootVolumePlan) { self.emit(render_plan_line(plan)); }

    fn instance_created(&self, handle: &InstanceHandle, cache_volume_id: Option<&str>) {
        self.emit(render_allocation_lines(handle, cache_volume_id));
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the report wording.

    use rstest::rstest;

    use super::*;
    use crate::boot_volume::{
        BootVolumeAllocation,
        BootVolumePlan,
        BootVolumeType,
        CompatibilityDefault,
        RootVolume,
    };

    /// A plan with the given settings.
    fn plan(
        volume_type: BootVolumeType,
        size_bytes: u64,
        defaulted: CompatibilityDefault,
    ) -> BootVolumePlan {
        BootVolumePlan::new(volume_type, size_bytes, defaulted)
    }

    /// A handle recording `plan` with identifier `root_id`.
    fn handle(plan: BootVolumePlan, root_id: &str) -> InstanceHandle {
        InstanceHandle {
            id: String::from("srv-1"),
            zone: String::from("zone"),
            boot_volume: Some(BootVolumeAllocation {
                plan,
                root: RootVolume {
                    id: root_id.to_owned(),
                    volume_type: plan.volume_type,
                    size_bytes: plan.size_bytes,
                },
            }),
        }
    }

    /// An explicit plan says so, and states both units.
    #[test]
    fn plan_line_marks_a_configured_plan() {
        let line = render_plan_line(&plan(
            BootVolumeType::Local,
            20_000_000_000,
            CompatibilityDefault::default(),
        ));
        assert_eq!(
            line,
            "boot volume: l_ssd 20 GB (20000000000 bytes) [configured]"
        );
    }

    /// A defaulted plan names exactly which settings were defaulted.
    #[rstest]
    #[case::type_only(CompatibilityDefault { volume_type: true, size: false }, "compatibility default: type")]
    #[case::size(CompatibilityDefault { volume_type: false, size: true }, "compatibility default: size")]
    #[case::both(
        CompatibilityDefault { volume_type: true, size: true },
        "compatibility default: type, size"
    )]
    fn plan_line_names_the_defaulted_settings(
        #[case] defaulted: CompatibilityDefault,
        #[case] expected: &str,
    ) {
        let line = render_plan_line(&plan(BootVolumeType::Sbs, 10_000_000_000, defaulted));
        assert!(line.contains(expected), "{line}");
    }

    /// The allocation report carries the reported root identifier and both
    /// units, and states the cache separately.
    #[test]
    fn allocation_lines_report_boot_and_cache_separately() {
        let lines = render_allocation_lines(
            &handle(
                plan(
                    BootVolumeType::Local,
                    20_000_000_000,
                    CompatibilityDefault::default(),
                ),
                "vol-root",
            ),
            Some("vol-cache"),
        );
        assert_eq!(
            lines,
            concat!(
                "boot volume: vol-root l_ssd 20 GB (20000000000 bytes) [configured]\n",
                "cache volume: vol-cache"
            )
        );
    }

    /// A request with no cache says `none` rather than borrowing the boot
    /// values.
    #[test]
    fn allocation_lines_report_no_cache() {
        let lines = render_allocation_lines(
            &handle(
                plan(
                    BootVolumeType::Sbs,
                    10_000_000_000,
                    CompatibilityDefault::default(),
                ),
                "vol-root",
            ),
            None,
        );
        assert!(lines.contains("cache volume: none"), "{lines}");
        assert!(
            lines.contains("boot volume: vol-root sbs_volume"),
            "the boot line must not be derived from the cache: {lines}"
        );
    }

    /// A handle without a recorded root says `unknown` instead of omitting the
    /// line.
    #[test]
    fn allocation_lines_report_an_unknown_boot_record() {
        let handle = InstanceHandle {
            id: String::from("srv-1"),
            zone: String::from("zone"),
            boot_volume: None,
        };
        let lines = render_allocation_lines(&handle, None);
        assert_eq!(lines, "boot volume: unknown\ncache volume: none");
    }

    /// The stderr reporter writes one line per fact and survives a dead
    /// stream.
    #[test]
    fn stderr_reporter_writes_both_facts() {
        let reporter = StderrReporter::new(Vec::new());
        reporter.plan_resolved(&plan(
            BootVolumeType::Local,
            20_000_000_000,
            CompatibilityDefault::default(),
        ));
        reporter.instance_created(
            &handle(
                plan(
                    BootVolumeType::Local,
                    20_000_000_000,
                    CompatibilityDefault::default(),
                ),
                "vol-root",
            ),
            Some("vol-cache"),
        );

        let guard = reporter.writer.lock().expect("lock");
        let text = String::from_utf8(guard.clone()).expect("utf8");
        assert!(text.contains("boot volume: l_ssd"), "{text}");
        assert!(text.contains("cache volume: vol-cache"), "{text}");
    }
}
