//! The per-run checks a campaign file must satisfy.
//!
//! These are separate from the file reader so each rule can be read and tested
//! on its own: a missing pin, a contradictory lifetime, a GPU record without
//! its accelerator details. The reader decides what a file contains; this
//! module decides whether what it contains is evidence.

use crate::bench::{
    record::{CacheState, GuestArchitecture, RunOutcome, RunRecord, WorkloadClass},
    validate::Complaint,
};

/// Reports every run record that cannot support a comparison.
pub(super) fn check_runs(runs: &[(usize, RunRecord)], complaints: &mut Vec<Complaint>) {
    if runs.is_empty() {
        complaints.push(Complaint::NoRuns);
    }
    for (line, run) in runs {
        report_missing_pins(*line, run, complaints);
        report_incoherence(*line, run, complaints);
    }
}

/// Names each pin the record leaves empty.
fn report_missing_pins(line: usize, run: &RunRecord, complaints: &mut Vec<Complaint>) {
    let pins: [(&'static str, bool); 16] = [
        ("campaign_id", run.campaign_id.trim().is_empty()),
        ("workload_id", run.workload_id.trim().is_empty()),
        ("command", run.command.is_empty()),
        ("tree.remote", run.tree.remote.trim().is_empty()),
        ("tree.commit", run.tree.commit.trim().is_empty()),
        ("tree.tree_digest", run.tree.tree_digest.trim().is_empty()),
        (
            "toolchain.compiler",
            run.toolchain.compiler.trim().is_empty(),
        ),
        (
            "toolchain.compiler_version",
            run.toolchain.compiler_version.trim().is_empty(),
        ),
        (
            "toolchain.package_manager",
            run.toolchain.package_manager.trim().is_empty(),
        ),
        (
            "toolchain.package_manager_version",
            run.toolchain.package_manager_version.trim().is_empty(),
        ),
        ("provider.provider", run.provider.provider.trim().is_empty()),
        ("provider.product", run.provider.product.trim().is_empty()),
        ("provider.location", run.provider.location.trim().is_empty()),
        (
            "allocation.guest_architecture",
            run.allocation.guest_architecture == GuestArchitecture::Other,
        ),
        ("allocation.cpu_count", run.allocation.cpu_count == 0),
        (
            "concurrency.intended_workers",
            run.concurrency.intended_workers == 0,
        ),
    ];
    for (field, missing) in pins {
        if missing {
            complaints.push(Complaint::MissingPin { line, field });
        }
    }
}

/// Reports the contradictions a driver or reader could not otherwise notice.
///
/// Each check exists because the alternative is a record that reads as evidence
/// but cannot be compared: a repetition zero, a chargeable lifetime shorter
/// than the attempt, or a GPU class without accelerator details.
fn report_incoherence(line: usize, run: &RunRecord, complaints: &mut Vec<Complaint>) {
    if run.repetition == 0 {
        complaints.push(Complaint::Incoherent {
            line,
            message: String::from("repetition numbers start at one"),
        });
    }
    // A provider bills at least the time measured, and legitimately bills more
    // when a granularity or a minimum period rounds up: a two-minute Scaleway
    // run is charged for an hour. Billing less than the measurement is the
    // contradiction, because it would mean the figure came from somewhere
    // other than this attempt.
    if run.lifetime.chargeable_nanos < run.lifetime.total_nanos {
        complaints.push(Complaint::Incoherent {
            line,
            message: String::from(
                "chargeable lifetime is shorter than the measured lifetime, so the record was \
                 not priced from its own measurement",
            ),
        });
    }
    if run.lifetime.total_nanos == 0 {
        complaints.push(Complaint::Incoherent {
            line,
            message: String::from("a completed attempt has a non-zero lifetime"),
        });
    }
    match (run.class, run.gpu.as_ref()) {
        (WorkloadClass::Gpu, None) => complaints.push(Complaint::Incoherent {
            line,
            message: String::from("a GPU experiment must record its accelerator details"),
        }),
        (WorkloadClass::Cpu, Some(_)) => complaints.push(Complaint::Incoherent {
            line,
            message: String::from("a CPU experiment must not carry accelerator details"),
        }),
        (WorkloadClass::Gpu, Some(gpu)) if !gpu.correctness_passed => {
            complaints.push(Complaint::Incoherent {
                line,
                message: String::from(
                    "a GPU experiment without a passing correctness check is a failure, not a \
                     ranking candidate",
                ),
            });
        }
        _ => {}
    }
    if claims_an_unmounted_cache(run) {
        complaints.push(Complaint::Incoherent {
            line,
            message: String::from(
                "a run that claims a cache state other than absent did not have it mounted",
            ),
        });
    }
    if run.kind != crate::bench::record::RecordKind::Run {
        complaints.push(Complaint::Incoherent {
            line,
            message: String::from("a run record carries the run kind tag"),
        });
    }
}

/// Returns whether a completed run claims a cache it did not have mounted.
fn claims_an_unmounted_cache(run: &RunRecord) -> bool {
    matches!(run.outcome, RunOutcome::Completed { .. })
        && !run.cache.mounted
        && run.cache.state != CacheState::Absent
}
