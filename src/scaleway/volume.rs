//! Volume attachment and mounting helpers for the Scaleway backend.

use std::collections::HashMap;

use serde::Serialize;

use crate::ownership::StorageFamily;

/// Volume reference for attachment in the Scaleway API.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct VolumeAttachment {
    /// Volume identifier (UUID).
    pub id: String,
    /// Whether this volume should be used for booting.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub boot: bool,
    /// Volume type, required by the provider for an SBS volume.
    ///
    /// The Instances API accepts local and Basic Block Storage volumes by
    /// identifier alone, but rejects an SBS volume attached without
    /// `"volume_type": "sbs_volume"`. Absent for the local families, so the
    /// request shape for them is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_type: Option<String>,
}

impl VolumeAttachment {
    /// Builds an attachment for `id`, naming the type only when required.
    pub(crate) fn new(id: String, boot: bool, family: StorageFamily) -> Self {
        let volume_type = match family {
            StorageFamily::Block => Some(String::from(SBS_VOLUME_TYPE)),
            StorageFamily::Instance => None,
        };
        Self {
            id,
            boot,
            volume_type,
        }
    }
}

/// Provider type name for a volume managed by the Block Storage API.
pub(crate) const SBS_VOLUME_TYPE: &str = "sbs_volume";

/// Request body for `PATCH /servers/{id}` to attach volumes.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct UpdateInstanceVolumesRequest {
    /// Volume map keyed by index ("0" for root, "1" for first additional, etc.).
    pub volumes: HashMap<String, VolumeAttachment>,
}

#[cfg(test)]
mod tests {
    //! Unit tests for volume attachment serialization.
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::omits_boot_when_false(false, r#"{"id":"vol-123"}"#)]
    #[case::includes_boot_when_true(true, r#"{"id":"vol-123","boot":true}"#)]
    fn volume_attachment_serializes_boot_only_when_true(
        #[case] boot: bool,
        #[case] expected: &str,
    ) {
        let attachment =
            VolumeAttachment::new(String::from("vol-123"), boot, StorageFamily::Instance);
        let json = serde_json::to_string(&attachment).expect("serialize");
        assert_eq!(json, expected);
    }

    #[rstest]
    fn an_sbs_attachment_names_its_volume_type() {
        let attachment =
            VolumeAttachment::new(String::from("vol-sbs"), false, StorageFamily::Block);
        let json = serde_json::to_string(&attachment).expect("serialize");
        assert_eq!(json, r#"{"id":"vol-sbs","volume_type":"sbs_volume"}"#);
    }

    #[rstest]
    fn update_request_serializes_volume_map() {
        let mut volumes = HashMap::new();
        volumes.insert(
            String::from("0"),
            VolumeAttachment::new(String::from("root-vol"), true, StorageFamily::Instance),
        );
        volumes.insert(
            String::from("1"),
            VolumeAttachment::new(String::from("cache-vol"), false, StorageFamily::Instance),
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
