//! Rendering a priced report as plain text.
//!
//! The renderer prints both cost figures and says which applies, rather than
//! printing one number under two headings. It also prints the capability
//! differences and comparability problems before the totals, so a reader sees
//! why a comparison is or is not valid before reading its result.

use super::{BenchmarkReport, WorkloadResult};
use crate::bench::billing::{NANOS_PER_SECOND, figures_agree, format_currency};

/// Renders the report as plain text.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
///
/// use mriya::bench::{billing::Totals, report::BenchmarkReport};
///
/// let report = BenchmarkReport {
///     campaign_id: String::from("c"),
///     currency: String::from("EUR"),
///     retrieved: String::from("2026-11-26"),
///     workloads: BTreeMap::new(),
///     capability_notes: Vec::new(),
///     comparability_problems: Vec::new(),
///     totals: Totals::default(),
///     subscription_micros: 0,
/// };
/// let rendered = render(&report);
/// assert!(rendered.contains("campaign c"));
/// ```
#[must_use]
pub fn render(report: &BenchmarkReport) -> String {
    let mut lines = vec![
        format!(
            "campaign {} priced in {} from a sheet read {}",
            report.campaign_id, report.currency, report.retrieved
        ),
        String::new(),
    ];

    if report.workloads.is_empty() {
        lines.push(String::from("no run records; nothing to report"));
    }
    for (workload_id, results) in &report.workloads {
        lines.push(format!("workload {workload_id}"));
        for result in results {
            lines.extend(render_result(report, result));
        }
        lines.push(String::new());
    }

    if !report.capability_notes.is_empty() {
        lines.push(String::from("capability differences"));
        for note in &report.capability_notes {
            lines.push(format!("- {note}"));
        }
        lines.push(String::new());
    }
    if !report.comparability_problems.is_empty() {
        lines.push(String::from("comparability problems"));
        for problem in &report.comparability_problems {
            lines.push(format!("- {problem}"));
        }
        lines.push(String::new());
    }

    lines.push(format!(
        "campaign marginal cost {}",
        format_currency(&report.currency, report.totals.marginal_micros)
    ));
    lines.push(format!(
        "campaign total-account cost {} of which subscription {}",
        format_currency(&report.currency, report.totals.total_account_micros),
        format_currency(&report.currency, report.subscription_micros)
    ));
    lines.push(String::new());
    lines.join("\n")
}

/// Renders one provider-product group.
fn render_result(report: &BenchmarkReport, result: &WorkloadResult) -> Vec<String> {
    let mut lines = vec![format!(
        "  {} / {} product {}: {} completed, {} failed",
        result.workload_id, result.provider, result.product, result.completed, result.failed
    )];
    lines.push(format!(
        "    cache {:?}, {} bytes; absorbed by plan {}; chargeable {}",
        result.cache_state,
        result.cache_bytes,
        format_duration(result.absorbed_nanos),
        format_duration(result.chargeable_nanos)
    ));
    if let Some(per_gate) = result.marginal_per_gate_micros() {
        lines.push(format!(
            "    marginal cost per completed gate {}",
            format_currency(&report.currency, per_gate)
        ));
    } else {
        lines.push(String::from(
            "    no completed gate, so no marginal cost per gate is reported",
        ));
    }
    if result.failed > 0 {
        lines.push(format!(
            "    failed-attempt cost {}",
            format_currency(&report.currency, result.failed_marginal_micros)
        ));
    }
    if let (Some(minimum), Some(median), Some(maximum)) = (
        result.minimum_execution_nanos(),
        result.median_execution_nanos(),
        result.maximum_execution_nanos(),
    ) {
        lines.push(format!(
            "    execution min {} median {} max {}",
            format_duration(minimum),
            format_duration(median),
            format_duration(maximum)
        ));
    }
    if !figures_agree(
        result.completed_marginal_micros,
        result.total_account_micros,
    ) {
        lines.push(format!(
            "    total-account cost of completed gates {}, including subscription share {}",
            format_currency(&report.currency, result.total_account_micros),
            format_currency(&report.currency, result.subscription_micros)
        ));
    }
    lines
}

/// Renders a duration as seconds with three decimals, without a float.
///
/// # Examples
///
/// ```
/// use mriya::bench::report::format_duration;
///
/// assert_eq!(format_duration(0), "0.000s");
/// assert_eq!(format_duration(1_500_000_000), "1.500s");
/// ```
#[must_use]
pub fn format_duration(nanos: u64) -> String {
    let seconds = nanos.checked_div(NANOS_PER_SECOND).unwrap_or(0);
    let millis = nanos
        .checked_rem(NANOS_PER_SECOND)
        .unwrap_or(0)
        .checked_div(1_000_000)
        .unwrap_or(0);
    format!("{seconds}.{millis:03}s")
}
