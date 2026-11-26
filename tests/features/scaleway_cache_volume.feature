Feature: Cache volume persistence across Scaleway VMs

  Scenario: A cache volume survives three successive VMs
    Given live Scaleway credentials and a test run id
    When I run the three-VM cache persistence check
    Then sentinel A is readable on the second VM
    And sentinel B is readable on the third VM
    And the volume was available after every VM
    And the volume is gone after deletion