//! Volume attachment and mounting helpers for the Scaleway backend.

use std::collections::HashMap;

use serde::Serialize;

/// Volume reference for attachment in the Scaleway API.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct VolumeAttachment {
    /// Volume identifier (UUID).
    pub id: String,
    /// Whether this volume should be used for booting.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub boot: bool,
}

/// Request body for `PATCH /servers/{id}` to attach volumes.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct UpdateInstanceVolumesRequest {
    /// Volume map keyed by index ("0" for root, "1" for first additional, etc.).
    pub volumes: HashMap<String, VolumeAttachment>,
}

#[cfg(test)]
mod tests {
    //! Unit tests for volume attachment serialization.
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::omits_boot_when_false(false, r#"{"id":"vol-123"}"#)]
    #[case::includes_boot_when_true(true, r#"{"id":"vol-123","boot":true}"#)]
    fn volume_attachment_serializes_boot_only_when_true(
        #[case] boot: bool,
        #[case] expected: &str,
    ) {
        let attachment = VolumeAttachment {
            id: String::from("vol-123"),
            boot,
        };
        let json = serde_json::to_string(&attachment).expect("serialize");
        assert_eq!(json, expected);
    }

    #[rstest]
    fn update_request_serializes_volume_map() {
        let mut volumes = HashMap::new();
        volumes.insert(
            String::from("0"),
            VolumeAttachment {
                id: String::from("root-vol"),
                boot: true,
            },
        );
        volumes.insert(
            String::from("1"),
            VolumeAttachment {
                id: String::from("cache-vol"),
                boot: false,
            },
        );
        let request = UpdateInstanceVolumesRequest { volumes };
        let json = serde_json::to_value(&request).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "volumes": {
                    "0": {"id": "root-vol", "boot": true},
                    "1": {"id": "cache-vol"},
                }
            })
        );
    }
}
