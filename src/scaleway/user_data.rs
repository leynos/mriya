//! Cloud-init user-data helpers for the Scaleway backend.
//!
//! Scaleway exposes instance user-data as a per-server key/value store. When
//! the key is set to `cloud-init`, the value is consumed by cloud-init on the
//! instance's first boot.

/// Reserved user-data key that Scaleway recognises for cloud-init payloads.
pub(crate) const CLOUD_INIT_USER_DATA_KEY: &str = "cloud-init";

pub(crate) fn user_data_url(zone: &str, server_id: &str, key: &str) -> String {
    format!("https://api.scaleway.com/instance/v1/zones/{zone}/servers/{server_id}/user_data/{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_user_data_url() {
        let url = user_data_url("fr-par-1", "server-123", CLOUD_INIT_USER_DATA_KEY);
        assert_eq!(
            url,
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/servers/server-123/user_data/cloud-init"
        );
    }
}
