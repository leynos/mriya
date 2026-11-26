Feature: mriya init prepares a cache volume

  Scenario: Prepare a cache volume and update configuration
    Given a ready init workflow
    And the formatter succeeds
    When I prepare the cache volume
    Then the init result is successful
    And the new volume is waited for before attachment
    And the formatter is torn down
    And the volume is formatted
    And the config is updated
    And the instance is destroyed

  Scenario: Surface formatting failures and still teardown
    Given a ready init workflow
    And the formatter fails with exit code "1"
    When I prepare the cache volume
    Then the init error kind is "format"
    And the owned volume is deleted
    And the instance is destroyed

  Scenario: Never delete a volume that was already configured
    Given a ready init workflow
    And configuration already contains a volume id
    When I prepare the cache volume
    Then the init error kind is "config"
    And no volume is deleted
    And the volume is not created

  Scenario: Reject overwriting an existing volume ID without force
    Given a ready init workflow
    And configuration already contains a volume id
    When I prepare the cache volume
    Then the init error kind is "config"
    And the volume is not created

  Scenario: Warn when an environment volume ID overrides the new file value
    Given a ready init workflow
    And an environment volume ID "vol-env" is effective
    And force overwrite is enabled
    And the formatter succeeds
    When I prepare the cache volume
    Then the init result is successful
    And the init warning names the environment override

  Scenario: Warn when the environment repeats the file volume ID
    Given a ready init workflow
    And configuration already contains a volume id
    And an environment volume ID "vol-existing" is effective
    And force overwrite is enabled
    And the formatter succeeds
    When I prepare the cache volume
    Then the init result is successful
    And the init warning names the environment override

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

  Scenario: Surface volume readiness failures and delete the owned volume
    Given a ready init workflow
    And volume readiness fails
    When I prepare the cache volume
    Then the init error kind is "volume"
    And the owned volume is deleted

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

  Scenario: Surface ordered-teardown failures after formatting
    Given a ready init workflow
    And the formatter succeeds
    And volume detachment fails
    When I prepare the cache volume
    Then the init error kind is "teardown"

  Scenario: Report the retained volume when rollback deletion fails
    Given a ready init workflow
    And the formatter fails with exit code "1"
    And volume deletion fails
    When I prepare the cache volume
    Then the init error kind is "format"
    And the init error names the retained volume

  Scenario: Delete the detached volume when the formatter destroy fails
    Given a ready init workflow
    And the formatter succeeds
    And teardown fails
    When I prepare the cache volume
    Then the init error kind is "teardown"
    And the owned volume is deleted

  Scenario: Report the retained volume when destroy and deletion both fail
    Given a ready init workflow
    And the formatter succeeds
    And teardown fails
    And volume deletion fails
    When I prepare the cache volume
    Then the init error kind is "teardown"
    And the init error names the retained volume

  Scenario: Surface teardown failures after success
    Given a ready init workflow
    And the formatter succeeds
    And teardown fails
    When I prepare the cache volume
    Then the init error kind is "teardown"
