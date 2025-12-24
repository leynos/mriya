Feature: mriya init prepares a cache volume

  Scenario: Prepare a cache volume and update configuration
    Given a ready init workflow
    And the formatter succeeds
    When I prepare the cache volume
    Then the init result is successful
    And the volume is formatted
    And the config is updated
    And the instance is destroyed

  Scenario: Surface formatting failures and still teardown
    Given a ready init workflow
    And the formatter fails with exit code "1"
    When I prepare the cache volume
    Then the init error kind is "format"
    And the instance is destroyed

  Scenario: Reject overwriting an existing volume ID without force
    Given a ready init workflow
    And configuration already contains a volume id
    When I prepare the cache volume
    Then the init error kind is "config"
    And the volume is not created

  Scenario: Allow overwriting an existing volume ID with force
    Given a ready init workflow
    And configuration already contains a volume id
    And force overwrite is enabled
    And the formatter succeeds
    When I prepare the cache volume
    Then the init result is successful
    And the config is updated

  Scenario: Surface volume creation failures
    Given a ready init workflow
    And volume creation fails
    When I prepare the cache volume
    Then the init error kind is "volume"

  Scenario: Surface provisioning failures
    Given a ready init workflow
    And instance provisioning fails
    When I prepare the cache volume
    Then the init error kind is "provision"

  Scenario: Surface readiness failures and still teardown
    Given a ready init workflow
    And instance readiness fails
    When I prepare the cache volume
    Then the init error kind is "wait"
    And the instance is destroyed

  Scenario: Surface detachment failures and still teardown
    Given a ready init workflow
    And the formatter succeeds
    And volume detachment fails
    When I prepare the cache volume
    Then the init error kind is "detach"
    And the instance is destroyed

  Scenario: Surface teardown failures after success
    Given a ready init workflow
    And the formatter succeeds
    And teardown fails
    When I prepare the cache volume
    Then the init error kind is "teardown"
