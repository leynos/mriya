//! What a campaign intends to do, and the limits it must obey.
//!
//! A plan is the operator's declaration: which workloads, on which providers,
//! with which limits. [`CampaignPlan::validate`] checks it before any
//! allocation, because every defect it catches is one that would otherwise be
//! discovered after money had been spent.

use super::ports::CampaignError;
use crate::bench::record::{CampaignHeader, CampaignLimits, RecordKind, SCHEMA_VERSION};

/// One attempt a campaign intends to make.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedAttempt {
    /// Workload identifier from the protocol's workload set.
    pub workload_id: String,
    /// Provider the attempt targets.
    pub provider: String,
    /// Product the attempt requests.
    pub product: String,
    /// Longest the attempt is allowed to live, in nanoseconds.
    pub planned_lifetime_nanos: u64,
    /// What the attempt is expected to cost, in micro-units of the sheet's
    /// currency.
    ///
    /// The harness enforces the stated budget against this declared figure
    /// before each allocation, because an actual figure does not exist until
    /// after the money is spent.
    pub estimated_micros: u64,
}

/// Everything a campaign must declare before it may allocate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CampaignPlan {
    /// Identifier every record carries.
    pub campaign_id: String,
    /// Operator-declared limits.
    pub limits: CampaignLimits,
    /// Price sheet the campaign expects to be priced against.
    pub price_sheet: Option<String>,
    /// Providers the campaign compares.
    pub providers: Vec<String>,
    /// Attempts, in the order they run.
    pub attempts: Vec<PlannedAttempt>,
}

impl CampaignPlan {
    /// Checks that the plan is internally consistent.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::InvalidPlan`] when the campaign has no attempts,
    /// a zero limit, an attempt longer than the declared maximum lifetime, or a
    /// total estimate beyond the declared budget. Each of those would otherwise
    /// be discovered only after money had been spent.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::{
    ///     campaign::{CampaignPlan, PlannedAttempt},
    ///     record::CampaignLimits,
    /// };
    ///
    /// let plan = CampaignPlan {
    ///     campaign_id: String::from("c"),
    ///     limits: CampaignLimits {
    ///         max_spend_micros: 10,
    ///         max_attempts: 1,
    ///         max_attempt_nanos: 10,
    ///     },
    ///     price_sheet: None,
    ///     providers: vec![String::from("scaleway")],
    ///     attempts: vec![PlannedAttempt {
    ///         workload_id: String::from("cold-build"),
    ///         provider: String::from("scaleway"),
    ///         product: String::from("DEV1-S"),
    ///         planned_lifetime_nanos: 5,
    ///         estimated_micros: 5,
    ///     }],
    /// };
    /// assert!(plan.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), CampaignError> {
        if self.campaign_id.trim().is_empty() {
            return Err(CampaignError::InvalidPlan(String::from(
                "the campaign needs an identifier",
            )));
        }
        if self.attempts.is_empty() {
            return Err(CampaignError::InvalidPlan(String::from(
                "the campaign needs at least one attempt",
            )));
        }
        if self.limits.declares_a_zero() {
            return Err(CampaignError::InvalidPlan(String::from(
                "every limit must be greater than zero",
            )));
        }
        if self.attempts.len() > usize::try_from(self.limits.max_attempts).unwrap_or(0) {
            return Err(CampaignError::InvalidPlan(format!(
                "the plan lists {} attempts but the limit is {}",
                self.attempts.len(),
                self.limits.max_attempts
            )));
        }
        let mut estimated = 0_u64;
        for attempt in &self.attempts {
            if attempt.planned_lifetime_nanos > self.limits.max_attempt_nanos {
                return Err(CampaignError::InvalidPlan(format!(
                    "attempt for {} plans to live longer than the declared maximum",
                    attempt.workload_id
                )));
            }
            estimated = estimated.saturating_add(attempt.estimated_micros);
        }
        if estimated > self.limits.max_spend_micros {
            return Err(CampaignError::InvalidPlan(format!(
                "the plan estimates {estimated} micro-units but the budget is {}",
                self.limits.max_spend_micros
            )));
        }
        Ok(())
    }

    /// Builds the header record a campaign file begins with.
    #[must_use]
    pub fn header(&self) -> CampaignHeader {
        CampaignHeader {
            schema: SCHEMA_VERSION,
            kind: RecordKind::CampaignHeader,
            campaign_id: self.campaign_id.clone(),
            limits: Some(self.limits),
            price_sheet: self.price_sheet.clone(),
            providers: self.providers.clone(),
        }
    }
}
