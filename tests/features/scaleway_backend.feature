Feature: Scaleway backend lifecycle

  Scenario: Provision and destroy minimal instance
    Given valid Scaleway credentials
    When I provision and tear down an instance from "Ubuntu 24.04 Noble Numbat"
    Then the backend reports a reachable public IPv4 address

  Scenario: Reject unknown instance type
    Given valid Scaleway credentials
    When I request instance type "NOT_A_TYPE"
    Then the request is rejected because the instance type is unavailable

  Scenario: Reject unknown image label
    Given valid Scaleway credentials
    When I request image label "mriya-nonexistent-image"
    Then the request is rejected because the image cannot be resolved
