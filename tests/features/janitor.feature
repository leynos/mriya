Feature: Scaleway test-run janitor

  Scenario: Delete tagged resources and verify clean state
    Given a configured janitor for project "project" and test run "run-1"
    And scw lists one tagged server and one tagged volume
    When I run the janitor sweep
    Then the janitor reports deleting 1 server and 1 volume
    And the janitor deletes servers without deleting attached volumes

  Scenario: Fail the sweep when resources remain
    Given a configured janitor for project "project" and test run "run-1"
    And scw lists a tagged server that remains after deletion
    When I run the janitor sweep
    Then the janitor reports a not-clean error

  Scenario: Reject configuration with a blank project
    Given a janitor configured for project "   " and test run "run-1"
    Then the janitor rejects the configuration because project_id is blank

  Scenario: Reject configuration with a blank test run
    Given a janitor configured for project "project" and test run "  "
    Then the janitor rejects the configuration because test_run_id is blank

