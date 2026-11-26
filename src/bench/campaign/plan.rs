//! What a campaign intends to do, and the limits it must obey.
//!
//! A plan is the operator's declaration: which workloads, on which providers,
//! with which limits. [`CampaignPlan::validate`] checks it before any
//! allocation, because every defect it catches is one that would otherwise be
//! discovered after money had been spent.

use serde::Deserialize;

use super::ports::CampaignError;
use crate::bench::record::{CampaignHeader, CampaignLimits, RecordKind, RunRole, SCHEMA_VERSION};

/// One attempt a campaign intends to make.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedAttempt {
    /// Workload identifier from the protocol's workload set.
    pub workload_id: String,
    /// Which repetition of the workload this is, counting from one.
    ///
    /// The plan carries it rather than the driver counting calls, so a
    /// campaign that is resumed cannot number two attempts the same.
    pub repetition: u32,
    /// Whether the attempt is measured or only warms the cache.
    ///
    /// A warm-up is recorded and costed, and excluded from the timing summary
    /// by its role, so a cache-populating run cannot inflate a per-gate figure.
    pub role: RunRole,
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
    ///         repetition: 1,
    ///         role: RunRole::Timed,
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
        if self.providers.is_empty() {
            return Err(CampaignError::InvalidPlan(String::from(
                "the campaign must declare the providers it compares",
            )));
        }
        for attempt in &self.attempts {
            if !self.providers.iter().any(|name| name == &attempt.provider) {
                return Err(CampaignError::InvalidPlan(format!(
                    "attempt for {} targets provider {:?}, which the campaign does not declare",
                    attempt.workload_id, attempt.provider
                )));
            }
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
            if attempt.repetition == 0 {
                return Err(CampaignError::InvalidPlan(format!(
                    "attempt for {} has repetition zero; repetitions start at one",
                    attempt.workload_id
                )));
            }
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

impl CampaignPlan {
    /// Reads a campaign plan from TOML.
    ///
    /// Every limit is required: an absent limit is a plan that did not declare
    /// what it may spend, and defaulting one would defeat the operator's
    /// declaration.
    ///
    /// # Errors
    ///
    /// Returns a message when the text is not a campaign plan, or when the plan
    /// it describes fails [`CampaignPlan::validate`].
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::campaign::CampaignPlan;
    ///
    /// let error = CampaignPlan::from_toml("[campaign]\nid = \"c\"\n").unwrap_err();
    /// assert!(error.contains("campaign is not readable"), "{error}");
    /// ```
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let raw: RawPlan =
            toml::from_str(text).map_err(|err| format!("campaign is not readable: {err}"))?;
        let plan = Self {
            campaign_id: raw.campaign.id,
            limits: CampaignLimits {
                max_spend_micros: raw.campaign.limits.spend_micros,
                max_attempts: raw.campaign.limits.attempts,
                max_attempt_nanos: raw.campaign.limits.attempt_nanos,
            },
            price_sheet: raw.campaign.price_sheet,
            providers: raw.campaign.providers,
            attempts: raw
                .attempts
                .into_iter()
                .map(|attempt| PlannedAttempt {
                    workload_id: attempt.workload_id,
                    repetition: attempt.repetition,
                    role: attempt.role,
                    provider: attempt.provider,
                    product: attempt.product,
                    planned_lifetime_nanos: attempt.planned_lifetime_nanos,
                    estimated_micros: attempt.estimated_micros,
                })
                .collect(),
        };
        plan.validate().map_err(|err| err.to_string())?;
        Ok(plan)
    }
}

/// Returns the repetition a campaign file assumes when it does not say.
const fn first_repetition() -> u32 { 1 }

/// The on-disk shape of a campaign plan.
#[derive(Debug, Deserialize)]
struct RawPlan {
    /// The campaign's identity and limits.
    campaign: RawCampaign,
    /// The attempts, in the order they run.
    #[serde(default)]
    attempts: Vec<RawAttempt>,
}

/// The on-disk shape of the `[campaign]` table.
#[derive(Debug, Deserialize)]
struct RawCampaign {
    /// Campaign identifier.
    id: String,
    /// Price sheet the campaign expects to be priced against.
    #[serde(default)]
    price_sheet: Option<String>,
    /// Providers the campaign compares.
    #[serde(default)]
    providers: Vec<String>,
    /// Operator-declared limits.
    limits: LimitsDocument,
}

/// The `[campaign.limits]` table.
///
/// Named so the three `max_` fields are judged as a group rather than flagged
/// as a repeated prefix, while the TOML keys stay as the protocol spells them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct LimitsDocument {
    /// Most the campaign may spend, in micro-units.
    #[serde(rename = "max_spend_micros")]
    spend_micros: u64,
    /// Most attempts the campaign may make.
    #[serde(rename = "max_attempts")]
    attempts: u32,
    /// Longest any one attempt may live, in nanoseconds.
    #[serde(rename = "max_attempt_nanos")]
    attempt_nanos: u64,
}

/// The on-disk shape of one `[[attempts]]` entry.
#[derive(Debug, Deserialize)]
struct RawAttempt {
    /// Workload identifier.
    workload_id: String,
    /// Which repetition of the workload this is, counting from one.
    #[serde(default = "first_repetition")]
    repetition: u32,
    /// Whether the attempt is measured or only warms the cache.
    #[serde(default)]
    role: RunRole,
    /// Provider the attempt targets.
    provider: String,
    /// Product the attempt requests.
    product: String,
    /// Longest the attempt is allowed to live, in nanoseconds.
    planned_lifetime_nanos: u64,
    /// Expected cost, in micro-units.
    estimated_micros: u64,
}
