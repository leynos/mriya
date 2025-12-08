Feature: Remote execution via mriya run

  Scenario: Propagate remote exit codes through the CLI orchestrator
    Given a ready backend and sync pipeline
    And the scripted runner returns exit code "7"
    When I orchestrate a remote run for "cargo test"
    Then the run result exit code is "7"
    And the instance is destroyed

  Scenario: Surface sync failures and still teardown
    Given a ready backend and sync pipeline
    And sync fails with status "12"
    When I orchestrate a remote run for "cargo test"
    Then the run error mentions sync failure
    And the instance is destroyed

  Scenario: Surface teardown failure after success
    Given a backend that fails during teardown
    And the scripted runner returns exit code "0"
    When I orchestrate a remote run for "echo ok"
    Then teardown failure is reported
