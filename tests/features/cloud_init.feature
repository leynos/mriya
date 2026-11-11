Feature: Cloud-init user-data configuration

  Scenario: Omit cloud-init user-data by default
    Given fake request dumping is enabled for cloud-init
    When I run mriya without cloud-init user-data
    Then the run request has cloud-init user-data present "false"
    And the run request has cloud-init user-data size "0"

  Scenario: Accept inline cloud-init user-data
    Given fake request dumping is enabled for cloud-init
    When I run mriya with inline cloud-init user-data "userdata-123"
    Then the run request has cloud-init user-data present "true"
    And the run request has cloud-init user-data size "12"

  Scenario: Accept file-based cloud-init user-data
    Given fake request dumping is enabled for cloud-init
    When I run mriya with cloud-init user-data file containing "file-user-data"
    Then the run request has cloud-init user-data present "true"
    And the run request has cloud-init user-data size "14"

  Scenario: Reject empty cloud-init user-data
    Given fake request dumping is enabled for cloud-init
    When I run mriya with inline cloud-init user-data "   "
    Then the run fails with error containing "invalid cloud-init configuration"

  Scenario: Reject missing cloud-init file
    Given fake request dumping is enabled for cloud-init
    When I run mriya with missing cloud-init file "does-not-exist.yml"
    Then the run fails with error containing "failed to read --cloud-init-file"

  Scenario: Expand a tilde cloud-init file path through HOME
    Given fake request dumping is enabled for cloud-init
    When I run mriya with cloud-init file "user-data.yml" relative to the temporary home
    Then the run request has cloud-init user-data present "true"
    And the run request has cloud-init user-data size "14"

  @unix
  Scenario: Report a tilde cloud-init path that HOME cannot decode
    Given fake request dumping is enabled for cloud-init
    When I run mriya with a tilde cloud-init file and a non-UTF-8 HOME
    Then the run fails with error containing "HOME is set to a value that is not valid UTF-8"
    And the run fails with error containing "~/user-data.yml"
