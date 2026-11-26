//! Tests for reading and validating a campaign plan.

use super::{super::campaign::CampaignPlan, support::example_campaign};

#[test]
fn the_example_campaign_plan_loads_and_validates() {
    let text = example_campaign();
    let plan = CampaignPlan::from_toml(&text).expect("the example plan should load");
    assert_eq!(plan.campaign_id, "example-2026-11-26");
    assert_eq!(plan.attempts.len(), 4);
    assert_eq!(
        plan.attempts
            .iter()
            .map(|attempt| attempt.workload_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "cold-build",
            "warm-incremental",
            "link-heavy",
            "test-execution",
        ]
    );
}

#[test]
fn a_plan_with_a_repetition_of_zero_is_refused() {
    let text = example_campaign().replace("repetition = 1", "repetition = 0");
    let error = CampaignPlan::from_toml(&text).expect_err("repetitions start at one");
    assert!(error.contains("repetition zero"), "{error}");
}
