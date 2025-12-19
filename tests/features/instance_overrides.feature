Feature: Instance type and image overrides

  Scenario: Use configured defaults when overrides are omitted
    Given fake request dumping is enabled
    When I run mriya without instance overrides
    Then the run request uses instance type "DEV1-S"
    And the run request uses image "Ubuntu 24.04 Noble Numbat"

  Scenario: Override instance type and image per run
    Given fake request dumping is enabled
    When I run mriya with instance type "DEV1-M" and image "Ubuntu 24.04 Noble Numbat"
    Then the run request uses instance type "DEV1-M"
    And the run request uses image "Ubuntu 24.04 Noble Numbat"

  Scenario: Reject empty instance type override
    Given fake request dumping is enabled
    When I run mriya with instance type "   " and image "Ubuntu 24.04 Noble Numbat"
    Then the run fails with error containing "invalid override for instance_type"

  Scenario: Reject empty image override
    Given fake request dumping is enabled
    When I run mriya with instance type "DEV1-S" and image "   "
    Then the run fails with error containing "invalid override for image"

