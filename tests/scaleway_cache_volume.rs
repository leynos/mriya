//! Opt-in live proof that a cache volume survives successive VMs.
//!
//! The suite is gated like `tests/scaleway_backend.rs`: it runs only when
//! `MRIYA_RUN_SCALEWAY_TESTS` is truthy and live credentials are present. It
//! additionally requires a nonblank `MRIYA_TEST_RUN_ID`, so every server and
//! volume carries a janitor tag and a failed run leaves sweepable resources.
//!
//! The flow writes sentinel A on VM 1, reads A and writes B on VM 2, and reads
//! B on VM 3. After every lifetime it checks that the server is gone and the
//! volume is `available`; it then deletes the volume and checks that it is
//! gone. This is the only test that exercises the real provider for the
//! detach-before-delete contract.

#[path = "scaleway_cache_volume_support/flow.rs"]
mod flow;
#[path = "scaleway_cache_volume_support/harness.rs"]
mod harness;
#[path = "scaleway_cache_volume_support/scripts.rs"]
mod scripts;

use flow::run_flow;
use harness::{
    CacheEvidence,
    CacheEvidenceSlot,
    CacheHarness,
    StepError,
    block_on,
    cache_evidence_slot,
    cache_harness,
};
use rstest_bdd::skip;
use rstest_bdd_macros::{given, scenario, then, when};

#[given("live Scaleway credentials and a test run id")]
fn live_credentials(cache_harness: Result<Option<CacheHarness>, String>) -> Result<(), String> {
    match cache_harness {
        Err(error) => Err(error),
        Ok(None) => skip!("live Scaleway cache suite not enabled"),
        Ok(Some(_)) => Ok(()),
    }
}

#[when("I run the three-VM cache persistence check")]
fn run_persistence(
    cache_harness: Result<Option<CacheHarness>, String>,
    cache_evidence_slot: CacheEvidenceSlot,
) -> Result<(), StepError> {
    let harness = match cache_harness {
        Ok(Some(harness)) => harness,
        Ok(None) => {
            return Err(StepError::Message(String::from(
                "live cache harness unavailable",
            )));
        }
        Err(error) => return Err(StepError::Message(error)),
    };
    let evidence =
        block_on(run_flow(&harness)).map_err(|error| StepError::Message(format!("{error:#}")))?;
    cache_evidence_slot.set(evidence);
    Ok(())
}

/// Returns the recorded evidence, failing the step when the flow did not run.
fn evidence(slot: &CacheEvidenceSlot) -> Result<CacheEvidence, StepError> {
    slot.get().ok_or_else(|| {
        StepError::Message(String::from(
            "the cache persistence check produced no evidence",
        ))
    })
}

#[then("sentinel A is readable on the second VM")]
fn sentinel_a_read(cache_evidence_slot: CacheEvidenceSlot) -> Result<(), StepError> {
    let evidence = evidence(&cache_evidence_slot)?;
    assert!(
        evidence.read_a_on_second,
        "VM 2 should read {}",
        evidence.sentinel_a
    );
    Ok(())
}

#[then("sentinel B is readable on the third VM")]
fn sentinel_b_read(cache_evidence_slot: CacheEvidenceSlot) -> Result<(), StepError> {
    let evidence = evidence(&cache_evidence_slot)?;
    assert!(
        evidence.read_b_on_third,
        "VM 3 should read {}",
        evidence.sentinel_b
    );
    Ok(())
}

#[then("the volume was available after every VM")]
fn volume_available(cache_evidence_slot: CacheEvidenceSlot) -> Result<(), StepError> {
    let evidence = evidence(&cache_evidence_slot)?;
    assert_eq!(
        evidence.volume_statuses,
        vec![
            String::from("available"),
            String::from("available"),
            String::from("available")
        ],
        "the volume should be available after each teardown"
    );
    Ok(())
}

#[then("the volume is gone after deletion")]
fn volume_gone(cache_evidence_slot: CacheEvidenceSlot) -> Result<(), StepError> {
    let evidence = evidence(&cache_evidence_slot)?;
    assert!(evidence.volume_gone, "the deleted volume should be gone");
    Ok(())
}

#[scenario(
    path = "tests/features/scaleway_cache_volume.feature",
    name = "A cache volume survives three successive VMs"
)]
fn scenario_cache_persistence(
    cache_harness: Result<Option<CacheHarness>, String>,
    cache_evidence_slot: CacheEvidenceSlot,
) {
    drop((cache_harness, cache_evidence_slot));
}
