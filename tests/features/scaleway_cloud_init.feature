Feature: Scaleway cloud-init provisioning

  Scenario: Cloud-init installs packages before the command runs
    Given valid Scaleway credentials and SSH sync configuration
    When I provision an instance with cloud-init installing jq and run "jq --version"
    Then the remote command succeeds and reports a jq version

