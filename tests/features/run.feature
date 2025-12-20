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

  Scenario: Mount cache volume before syncing when volume ID is configured
    Given a ready backend and sync pipeline
    And a volume ID "vol-12345" is configured
    And the scripted runner returns exit code "0"
    When I orchestrate a remote run for "cargo build"
    Then the run result exit code is "0"
    And the instance is destroyed

  Scenario: Continue execution when volume mount fails
    Given a ready backend and sync pipeline
    And a volume ID "vol-12345" is configured
    And the mount command fails
    And the scripted runner returns exit code "0"
    When I orchestrate a remote run for "cargo build"
    Then the run result exit code is "0"
    And the instance is destroyed

  Scenario: Route Cargo caches to the mounted cache volume
    Given a ready backend and sync pipeline
    And a volume ID "vol-12345" is configured
    And the scripted runner returns exit code "0"
    When I orchestrate a remote run for "cargo test"
    Then the remote command routes Cargo caches to the volume
    And the instance is destroyed

  Scenario: Allow disabling cache routing
    Given a ready backend and sync pipeline
    And cache routing is disabled
    And a volume ID "vol-12345" is configured
    And the scripted runner returns exit code "0"
    When I orchestrate a remote run for "cargo test"
    Then the remote command does not route Cargo caches
    And the instance is destroyed

  Scenario: Wait for cloud-init readiness before executing the remote command
    Given a ready backend and sync pipeline
    And cloud-init user data is configured
    And the rsync step succeeds
    And cloud-init is already finished
    And the remote command returns exit code "0"
    When I orchestrate a remote run for "echo ok"
    Then cloud-init readiness is checked before executing the remote command
    And the run result exit code is "0"
    And the instance is destroyed

  Scenario: Surface cloud-init provisioning check failures and still teardown
    Given a ready backend and sync pipeline
    And cloud-init user data is configured
    And the rsync step succeeds
    When I orchestrate a remote run for "echo ok"
    Then the run error mentions provisioning failure
    And the instance is destroyed

  Scenario: Surface cloud-init provisioning timeout and still teardown
    Given a ready backend and sync pipeline
    And a backend that fails during teardown
    And cloud-init user data is configured
    And the rsync step succeeds
    And cloud-init provisioning times out
    When I orchestrate a remote run for "echo ok"
    Then the run error is a provisioning timeout
    And the run error includes a teardown failure note
    And the instance is destroyed
