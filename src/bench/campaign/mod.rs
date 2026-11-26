//! The paid campaign harness, gated behind an explicit operator opt-in.
//!
//! Paid execution is the one thing in this repository that spends money, so the
//! harness is built around three refusals rather than three features:
//!
//! 1. It refuses to start unless the operator has set [`crate::bench::record::OPT_IN_ENV`] to
//!    [`crate::bench::record::OPT_IN_VALUE`] and declared every limit.
//! 2. It refuses to allocate until a pre-sweep has found nothing left behind by an earlier
//!    campaign.
//! 3. On any cleanup doubt it appends a residual record naming every identifier it could not verify
//!    gone, and stops. It never continues to the next attempt in the hope that the problem was
//!    transient.
//!
//! Every collaborator is injected, so the policy above is testable without a
//! provider, a credential, or a spend. The production implementations live in
//! the `mriya-bench` binary and in [`ScalewayDriver`].

mod plan;
mod ports;
mod runner;
mod scaleway_driver;
mod scaleway_measure;

pub use plan::{CampaignPlan, PlannedAttempt};
pub use ports::{
    AttemptFailure,
    AttemptRecords,
    CampaignDriver,
    CampaignError,
    JsonLinesSink,
    RecordSink,
    SweepReport,
    TeardownReport,
    require_opt_in,
};
pub use runner::run_campaign;
pub use scaleway_driver::ScalewayDriver;
#[cfg(test)]
pub(crate) use scaleway_measure::{redact_remote, shell_escape, tree_size};
