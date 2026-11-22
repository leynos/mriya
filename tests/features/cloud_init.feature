Feature: Cloud-init user-data configuration

  Scenario: Reject empty cloud-init user-data
    Given the ordinary mriya binary
    When I run mriya with inline cloud-init user-data "   "
    Then the run fails with error containing "invalid cloud-init configuration"

  Scenario: Reject missing cloud-init file
    Given the ordinary mriya binary
    When I run mriya with missing cloud-init file "does-not-exist.yml"
    Then the run fails with error containing "failed to read --cloud-init-file"

  # Every isolated run stops at the control-character check, so reaching that
  # error proves the tilde path was expanded and the file was read: a path that
  # did not resolve is reported as an unreadable file instead.
  Scenario: Read a tilde cloud-init file path through HOME
    Given the ordinary mriya binary
    When I run mriya with cloud-init file "user-data.yml" relative to the temporary home
    Then the run fails with error containing "invalid command argument"

  @unix
  Scenario: Report a tilde cloud-init path that HOME cannot decode
    Given the ordinary mriya binary
    When I run mriya with a tilde cloud-init file and a non-UTF-8 HOME
    Then the run fails with error containing "HOME is set to a value that is not valid UTF-8"
    And the run fails with error containing "~/user-data.yml"