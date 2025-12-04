Feature: Git-aware file sync

  Scenario: Preserve gitignored caches on the remote
    Given a workspace with a gitignored cache on the remote
    When I run git-aware rsync sync to the remote path
    Then the gitignored cache directory remains after sync
    And tracked files are mirrored to the remote

  Scenario: Propagate remote exit codes
    Given a scripted runner that succeeds at sync
    When the remote command exits with "7"
    Then the orchestrator reports exit code "7"

  Scenario: Surface sync failures
    Given a scripted runner that fails during sync
    When I trigger sync against the workspace
    Then the sync error mentions the rsync exit code
