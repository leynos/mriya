//! Regression test for step return propagation under `rstest-bdd` 0.6.
//!
//! Before 0.6 a step could return an aliased `Err` and its scenario still
//! passed. These scenarios pin the corrected behaviour in both directions: an
//! aliased `Err` fails the scenario, an aliased `Ok(T)` injects `T` for a later
//! step to read, and an aliased `Ok(())` injects nothing.
//!
//! The feature file sits in `tests/regression_features/`, outside
//! `tests/features/`, because its first scenario fails by design. The test
//! that loads it is `#[should_panic]` with the step's error text, so a
//! scenario that wrongly continues past the failing step panics with a
//! different message and turns the test red.

use rstest::fixture;
use rstest_bdd_macros::{given, scenario, then, when};

/// Value type injected by the step-return rules; unique among the fixtures.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Marker(u32);

/// Deliberate failure raised by the step under test.
#[derive(Debug, thiserror::Error)]
#[error("deliberate step failure")]
struct DeliberateFailure;

/// Local alias of `Result`, the form 0.6 classifies as fallible.
type Outcome<T> = Result<T, DeliberateFailure>;

#[mriya_test_macros::allow_fixture_expansion_lints]
#[fixture]
fn marker() -> Marker { Marker(3) }

#[given("a step returns an aliased Err")]
fn step_returns_aliased_err() -> Outcome<()> { Err(DeliberateFailure) }

#[then("the scenario never reaches this step")]
fn scenario_never_reaches_this_step() {
    panic!("step propagation is neutralized: the scenario continued after an aliased Err");
}

/// Parses the placeholder so the step is genuinely fallible, not a constant.
fn parse_number(text: &str) -> Outcome<u32> { text.parse().map_err(|_| DeliberateFailure) }

#[when("a step returns an aliased Ok carrying {number}")]
fn step_returns_aliased_ok_value(number: String) -> Outcome<Marker> {
    parse_number(&number).map(Marker)
}

#[then("the marker fixture holds 7")]
fn marker_holds_injected_value(marker: &Marker) {
    assert_eq!(*marker, Marker(7));
}

#[when("a step returns an aliased Ok of unit after parsing {number}")]
fn step_returns_aliased_ok_unit(number: String) -> Outcome<()> { parse_number(&number).map(drop) }

#[then("the marker fixture still holds 3")]
fn marker_keeps_original_value(marker: &Marker) {
    assert_eq!(*marker, Marker(3));
}

#[scenario(
    path = "tests/regression_features/step_return_propagation.feature",
    name = "A failing aliased step result fails the scenario"
)]
#[should_panic(expected = "deliberate step failure")]
fn aliased_err_fails_the_scenario(_marker: Marker) {}

#[scenario(
    path = "tests/regression_features/step_return_propagation.feature",
    name = "A passing aliased step result injects its value"
)]
fn aliased_ok_injects_its_value(_marker: Marker) {}

#[scenario(
    path = "tests/regression_features/step_return_propagation.feature",
    name = "A unit aliased step result injects nothing"
)]
fn aliased_ok_unit_injects_nothing(_marker: Marker) {}
