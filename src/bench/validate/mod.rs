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

mod run_checks;

use run_checks::check_runs;

use super::record::{CampaignHeader, Record, RunRecord, SCHEMA_VERSION, parse_line};

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
            Self::Unreadable { line, message } | Self::Incoherent { line, message } => {
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
    let mut buckets = Buckets::default();

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
                collect(record, line_number, &mut buckets, &mut complaints);
            }
            Err(err) => complaints.push(Complaint::Unreadable {
                line: line_number,
                message: err.to_string(),
            }),
        }
    }

    let header = judge_headers(&buckets.headers, &mut complaints);
    check_campaign_identity(
        header.as_ref(),
        &buckets.runs,
        &buckets.phases,
        &mut complaints,
    );
    check_runs(&buckets.runs, &mut complaints);

    if let Some(single) = header
        && complaints.is_empty()
    {
        return Ok(Campaign {
            header: single,
            runs: buckets.runs,
            phases: buckets.phases,
        });
    }
    if complaints.is_empty() {
        complaints.push(Complaint::NoRuns);
    }
    Err(complaints)
}

/// Files one parsed record into the bucket its kind belongs to.
fn collect(record: Record, line: usize, buckets: &mut Buckets, complaints: &mut Vec<Complaint>) {
    match record {
        Record::Header(header) => buckets.headers.push((line, *header)),
        Record::Run(run) => buckets.runs.push((line, *run)),
        Record::Phase(phase) => buckets.phases.push((line, *phase)),
        Record::Residual(residual) => complaints.push(Complaint::CleanupDoubt {
            line,
            resource_ids: residual.resource_ids,
        }),
    }
}

/// The records a campaign file's lines are sorted into.
#[derive(Default)]
struct Buckets {
    /// Campaign headers, with their line numbers.
    headers: Vec<(usize, CampaignHeader)>,
    /// Run records, with their line numbers.
    runs: Vec<(usize, RunRecord)>,
    /// Phase records, with their line numbers.
    phases: Vec<(usize, super::record::PhaseRecord)>,
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
    let Some(expected) = header else {
        return;
    };
    let mut foreign = BTreeSet::new();
    for (_, run) in runs {
        if run.campaign_id != expected.campaign_id {
            foreign.insert(run.campaign_id.clone());
        }
    }
    for (_, phase) in phases {
        if phase.campaign_id != expected.campaign_id {
            foreign.insert(phase.campaign_id.clone());
        }
    }
    if !foreign.is_empty() {
        complaints.push(Complaint::MixedCampaigns {
            expected: expected.campaign_id.clone(),
            found: foreign.into_iter().collect(),
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
