//! Offline validation of a campaign file.
//!
//! The validator is the part of the protocol that runs without a provider, a
//! credential, or a clock. It reads a JSON Lines campaign into [`Record`]s and
//! reports every defect it finds by name, rather than stopping at the first:
//! an operator fixing a campaign should see the whole list once.
//!
//! A file that validates clean is still not evidence of a cheap backend. It is
//! evidence that the records can be compared under the protocol, which is the
//! precondition for the report.

use std::{collections::BTreeSet, fmt};

use super::record::{
    CampaignHeader,
    GuestArchitecture,
    Record,
    RecordKind,
    RunOutcome,
    RunRecord,
    SCHEMA_VERSION,
    WorkloadClass,
    parse_line,
};

/// One reason a campaign file is not acceptable evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Complaint {
    /// A line could not be read at all.
    Unreadable {
        /// One-based line number.
        line: usize,
        /// Why the line could not be read.
        message: String,
    },
    /// A record declares a schema version this build does not read.
    SchemaMismatch {
        /// One-based line number.
        line: usize,
        /// The version the record declares.
        found: u32,
    },
    /// The file has no campaign header.
    MissingHeader,
    /// The file has more than one campaign header.
    RepeatedHeader {
        /// One-based line numbers where headers appear.
        lines: Vec<usize>,
    },
    /// The file has no run records, so it cannot support any claim.
    NoRuns,
    /// Two records disagree about the campaign they belong to.
    MixedCampaigns {
        /// The identifier the header declares.
        expected: String,
        /// The identifiers that disagree with it.
        found: Vec<String>,
    },
    /// A required pin is empty or absent.
    MissingPin {
        /// One-based line number.
        line: usize,
        /// The dotted name of the missing pin.
        field: &'static str,
    },
    /// A run record does not describe a coherent experiment.
    Incoherent {
        /// One-based line number.
        line: usize,
        /// What is wrong, in one sentence.
        message: String,
    },
    /// A residual resource means the campaign stopped on a cleanup doubt.
    CleanupDoubt {
        /// One-based line number of the residual record.
        line: usize,
        /// Identifiers that could not be verified gone.
        resource_ids: Vec<String>,
    },
}

impl fmt::Display for Complaint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { line, message } => {
                write!(formatter, "line {line}: {message}")
            }
            Self::SchemaMismatch { line, found } => write!(
                formatter,
                "line {line}: schema {found} is not readable by this build (expected {SCHEMA_VERSION})"
            ),
            Self::MissingHeader => formatter.write_str("no campaign header record"),
            Self::RepeatedHeader { lines } => {
                write!(formatter, "campaign header repeated on lines {lines:?}")
            }
            Self::NoRuns => formatter.write_str("no run records; the file proves nothing"),
            Self::MixedCampaigns { expected, found } => write!(
                formatter,
                "records name campaigns other than {expected:?}: {found:?}"
            ),
            Self::MissingPin { line, field } => {
                write!(formatter, "line {line}: missing pin {field}")
            }
            Self::Incoherent { line, message } => {
                write!(formatter, "line {line}: {message}")
            }
            Self::CleanupDoubt { line, resource_ids } => write!(
                formatter,
                "line {line}: campaign stopped with unverified resources {resource_ids:?}"
            ),
        }
    }
}

/// A campaign file that passed validation, with its records and line numbers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Campaign {
    /// The campaign header that governs the file.
    pub header: CampaignHeader,
    /// Every run record, paired with the line it came from.
    pub runs: Vec<(usize, RunRecord)>,
    /// Every phase record, paired with the line it came from.
    pub phases: Vec<(usize, super::record::PhaseRecord)>,
}

impl Campaign {
    /// Returns the campaign identifier the header declares.
    #[must_use]
    pub fn id(&self) -> &str { &self.header.campaign_id }
}

/// Validates a campaign file's text, returning the parsed campaign when clean.
///
/// Every complaint is reported, so the returned list is empty exactly when the
/// file is acceptable evidence.
///
/// # Errors
///
/// Returns the full complaint list when the text is not an acceptable campaign
/// file. The list is never empty on that path, and the first complaint is
/// always about the earliest problem in file order.
///
/// # Examples
///
/// ```
/// use mriya::bench::validate::validate_str;
///
/// let complaints = validate_str("").unwrap_err();
/// assert!(
///     complaints
///         .iter()
///         .any(|c| c.to_string().contains("no campaign header"))
/// );
/// assert!(
///     complaints
///         .iter()
///         .any(|c| c.to_string().contains("no run records"))
/// );
/// ```
pub fn validate_str(text: &str) -> Result<Campaign, Vec<Complaint>> {
    let mut complaints = Vec::new();
    let mut headers = Vec::new();
    let mut runs = Vec::new();
    let mut phases = Vec::new();

    for (index, line) in text.lines().enumerate() {
        let line_number = index.saturating_add(1);
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            Ok(record) => {
                if record.schema() != SCHEMA_VERSION {
                    complaints.push(Complaint::SchemaMismatch {
                        line: line_number,
                        found: record.schema(),
                    });
                    continue;
                }
                collect(
                    record,
                    line_number,
                    &mut headers,
                    &mut runs,
                    &mut phases,
                    &mut complaints,
                );
            }
            Err(err) => complaints.push(Complaint::Unreadable {
                line: line_number,
                message: err.to_string(),
            }),
        }
    }

    let header = judge_headers(&headers, &mut complaints);
    check_campaign_identity(header.as_ref(), &runs, &phases, &mut complaints);
    check_runs(&runs, &mut complaints);

    match (header, complaints.is_empty()) {
        (Some(header), true) => Ok(Campaign {
            header,
            runs,
            phases,
        }),
        _ => {
            if complaints.is_empty() {
                complaints.push(Complaint::NoRuns);
            }
            Err(complaints)
        }
    }
}

/// Files one parsed record into the bucket its kind belongs to.
fn collect(
    record: Record,
    line: usize,
    headers: &mut Vec<(usize, CampaignHeader)>,
    runs: &mut Vec<(usize, RunRecord)>,
    phases: &mut Vec<(usize, super::record::PhaseRecord)>,
    complaints: &mut Vec<Complaint>,
) {
    match record {
        Record::Header(header) => headers.push((line, *header)),
        Record::Run(run) => runs.push((line, *run)),
        Record::Phase(phase) => phases.push((line, *phase)),
        Record::Residual(residual) => complaints.push(Complaint::CleanupDoubt {
            line,
            resource_ids: residual.resource_ids,
        }),
    }
}

/// Reports header problems and returns the single header when there is one.
fn judge_headers(
    headers: &[(usize, CampaignHeader)],
    complaints: &mut Vec<Complaint>,
) -> Option<CampaignHeader> {
    match headers {
        [] => {
            complaints.push(Complaint::MissingHeader);
            None
        }
        [(_, header)] => Some(header.clone()),
        many => {
            complaints.push(Complaint::RepeatedHeader {
                lines: many.iter().map(|(line, _)| *line).collect(),
            });
            None
        }
    }
}

/// Requires every record to name the campaign the header declares.
fn check_campaign_identity(
    header: Option<&CampaignHeader>,
    runs: &[(usize, RunRecord)],
    phases: &[(usize, super::record::PhaseRecord)],
    complaints: &mut Vec<Complaint>,
) {
    let Some(header) = header else {
        return;
    };
    let mut foreign = BTreeSet::new();
    for (_, run) in runs {
        if run.campaign_id != header.campaign_id {
            foreign.insert(run.campaign_id.clone());
        }
    }
    for (_, phase) in phases {
        if phase.campaign_id != header.campaign_id {
            foreign.insert(phase.campaign_id.clone());
        }
    }
    if !foreign.is_empty() {
        complaints.push(Complaint::MixedCampaigns {
            expected: header.campaign_id.clone(),
            found: foreign.into_iter().collect(),
        });
    }
}

/// Reports every run record that cannot support a comparison.
fn check_runs(runs: &[(usize, RunRecord)], complaints: &mut Vec<Complaint>) {
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
    if matches!(run.outcome, RunOutcome::Completed { .. })
        && !run.cache.mounted
        && run.cache.state != super::record::CacheState::Absent
    {
        complaints.push(Complaint::Incoherent {
            line,
            message: String::from(
                "a run that claims a cache state other than absent did not have it mounted",
            ),
        });
    }
    if run.kind != RecordKind::Run {
        complaints.push(Complaint::Incoherent {
            line,
            message: String::from("a run record carries the run kind tag"),
        });
    }
}

/// Formats a complaint list for an operator, one complaint per line.
///
/// # Examples
///
/// ```
/// use mriya::bench::validate::{Complaint, format_complaints};
///
/// let rendered = format_complaints(&[Complaint::MissingHeader]);
/// assert_eq!(rendered, "no campaign header record");
/// ```
#[must_use]
pub fn format_complaints(complaints: &[Complaint]) -> String {
    complaints
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
