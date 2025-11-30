//! Unit-level tests for Scaleway backend helper error variants.

use mriya::ScalewayBackendError;

#[test]
fn missing_public_ip_error_variant_available() {
    let error = ScalewayBackendError::MissingPublicIp {
        instance_id: String::from("instance-id"),
    };
    assert_eq!(
        error.to_string(),
        "instance instance-id missing public IPv4 address"
    );
}

#[test]
fn power_on_not_allowed_error_variant_available() {
    let error = ScalewayBackendError::PowerOnNotAllowed {
        instance_id: String::from("instance-id"),
        state: String::from("stopped"),
    };
    assert_eq!(
        error.to_string(),
        "instance instance-id in state stopped cannot be powered on"
    );
}
