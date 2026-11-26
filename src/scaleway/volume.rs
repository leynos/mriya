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
    /// Provider volume type.
    ///
    /// The API requires it when the entry refers to an `sbs_volume`, and
    /// rejects it for a local volume, so it is omitted unless the entry really
    /// is Block Storage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_type: Option<String>,
}

impl VolumeAttachment {
    /// Builds a root entry, carrying the volume type only for Block Storage.
    ///
    /// For example, an `l_ssd` root renders `{"id": ..., "boot": true}` while
    /// an `sbs_volume` root adds `"volume_type": "sbs_volume"`.
    pub(crate) fn boot_root(
        id: String,
        volume_type: Option<crate::boot_volume::BootVolumeType>,
    ) -> Self {
        Self {
            id,
            boot: true,
            volume_type: volume_type
                .filter(|kind| *kind == crate::boot_volume::BootVolumeType::Sbs)
                .map(|kind| kind.provider_id().to_owned()),
        }
    }

    /// Builds a non-boot entry, which is always left without a volume type so
    /// existing local attachments keep their current payload.
    ///
    /// For example, the cache volume at index `"1"` renders `{"id": ...}`.
    pub(crate) const fn cache(id: String) -> Self {
        Self {
            id,
            boot: false,
            volume_type: None,
        }
    }
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
    use rstest::rstest;

    use super::*;

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
            volume_type: None,
        };
        let json = serde_json::to_string(&attachment).expect("serialize");
        assert_eq!(json, expected);
    }

    /// A local root keeps the historical payload, while an SBS root carries
    /// the type the API requires for Block Storage.
    #[rstest]
    #[case::local(
        Some(crate::boot_volume::BootVolumeType::Local),
        r#"{"id":"root-vol","boot":true}"#
    )]
    #[case::sbs(
        Some(crate::boot_volume::BootVolumeType::Sbs),
        r#"{"id":"root-vol","boot":true,"volume_type":"sbs_volume"}"#
    )]
    #[case::unknown(None, r#"{"id":"root-vol","boot":true}"#)]
    fn boot_root_serializes_the_volume_type_only_for_sbs(
        #[case] volume_type: Option<crate::boot_volume::BootVolumeType>,
        #[case] expected: &str,
    ) {
        let attachment = VolumeAttachment::boot_root(String::from("root-vol"), volume_type);
        let json = serde_json::to_string(&attachment).expect("serialize");
        assert_eq!(json, expected);
    }

    #[test]
    fn update_request_serializes_volume_map() {
        let mut volumes = HashMap::new();
        volumes.insert(
            String::from("0"),
            VolumeAttachment::boot_root(
                String::from("root-vol"),
                Some(crate::boot_volume::BootVolumeType::Local),
            ),
        );
        volumes.insert(
            String::from("1"),
            VolumeAttachment::cache(String::from("cache-vol")),
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
