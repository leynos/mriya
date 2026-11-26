//! Backend cost benchmark: protocol records, validation, and billing.
//!
//! This module is the offline half of the benchmark described in
//! `docs/backend-cost-benchmark-protocol.md`. It can read a campaign file,
//! report every way that file fails the protocol, and price an accepted record
//! against a source-cited sheet — all without a provider, a credential, or a
//! clock.
//!
//! Nothing here allocates anything. The paid path lives in `src/bench/campaign.rs`
//! and is reachable only through the `mriya-bench` binary with the operator
//! opt-in set.
//!
//! # Examples
//!
//! ```
//! use mriya::bench::validate::validate_str;
//!
//! // An empty campaign cannot support any claim, and says so twice.
//! let complaints = validate_str("").unwrap_err();
//! assert_eq!(complaints.len(), 2);
//! ```

pub mod billing;
pub mod observe;
pub mod record;
pub mod validate;

#[cfg(test)]
mod tests;
