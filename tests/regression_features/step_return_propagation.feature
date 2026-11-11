Feature: Step return propagation under rstest-bdd 0.6

  This file lives outside tests/features on purpose: its first scenario is
  meant to fail, and the test that loads it asserts that failure.

  Scenario: A failing aliased step result fails the scenario
    Given a step returns an aliased Err
    Then the scenario never reaches this step

  Scenario: A passing aliased step result injects its value
    When a step returns an aliased Ok carrying 7
    Then the marker fixture holds 7

  Scenario: A unit aliased step result injects nothing
    When a step returns an aliased Ok of unit after parsing 5
    Then the marker fixture still holds 3
