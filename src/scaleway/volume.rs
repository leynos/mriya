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
    use super::*;

    #[test]
    fn volume_attachment_serialises_without_boot_when_false() {
        let attachment = VolumeAttachment {
            id: String::from("vol-123"),
            boot: false,
        };
        let json = serde_json::to_string(&attachment).expect("serialise");
        assert!(!json.contains("boot"));
    }

    #[test]
    fn volume_attachment_serialises_with_boot_when_true() {
        let attachment = VolumeAttachment {
            id: String::from("vol-123"),
            boot: true,
        };
        let json = serde_json::to_string(&attachment).expect("serialise");
        assert!(json.contains(r#""boot":true"#));
    }

    #[test]
    fn update_request_serialises_volume_map() {
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
        let json = serde_json::to_string(&request).expect("serialise");
        assert!(json.contains(r#""volumes""#));
        assert!(json.contains("root-vol"));
        assert!(json.contains("cache-vol"));
    }
}
