Feature: Instance type and image overrides

  Scenario: Reject empty instance type override
    Given the ordinary mriya binary
    When I run mriya with instance type "   " and image "Ubuntu 24.04 Noble Numbat"
    Then the run fails with error containing "invalid override for --instance-type"

  Scenario: Reject empty image override
    Given the ordinary mriya binary
    When I run mriya with instance type "DEV1-S" and image "   "
    Then the run fails with error containing "invalid override for --image"