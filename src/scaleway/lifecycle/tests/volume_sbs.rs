//! Block Storage attachment tests through a loopback Scaleway API.
//!
//! The Instances API requires `volume_type` on an `sbs_volume` entry and
//! rejects it on a local one, so the root's own reported type decides whether
//! the field is present. These cases pin both directions.

use anyhow::{Context, Result};
use rstest::rstest;

use super::{
    backend_fixture,
    handle,
    http_stub::{Reply, only_request, request_body, with_stub},
};
use crate::{
    backend::InstanceHandle,
    boot_volume::{BootVolumeType, RootVolume},
    scaleway::ScalewayBackend,
};

/// A provider response confirming a volume patch.
fn patched() -> Reply {
    Reply {
        status: 200,
        body: String::from("{}"),
    }
}

/// A Block Storage root volume, which the PATCH must label.
fn sbs_root() -> RootVolume {
    RootVolume {
        id: String::from("vol-root"),
        volume_type: BootVolumeType::Sbs,
        size_bytes: 10_000_000_000,
    }
}

/// Scenario: attaching a cache volume to an instance with an SBS root.
///
/// Invariant: the root entry keeps its boot flag and gains
/// `volume_type: "sbs_volume"`, which the Instances API requires for a Block
/// Storage entry, while the cache entry stays unlabelled.
#[rstest]
#[tokio::test]
async fn attach_volume_labels_an_sbs_root(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(backend_fixture, vec![patched()], async |backend| {
        backend
            .attach_volume(&handle, "vol-cache", &sbs_root())
            .await
    })
    .await?;
    result?;
    assert_eq!(
        request_body(only_request(&requests)?)?,
        serde_json::json!({
            "volumes": {
                "0": {
                    "id": "vol-root",
                    "boot": true,
                    "volume_type": "sbs_volume",
                },
                "1": {"id": "vol-cache"},
            }
        })
    );
    Ok(())
}

/// Scenario: detaching a cache volume from an instance with an SBS root.
///
/// Invariant: detaching preserves the root entry with its Block Storage type,
/// so the provider does not reject the replacement payload for a missing type.
#[rstest]
#[tokio::test]
async fn detach_volume_labels_an_sbs_root(
    backend_fixture: ScalewayBackend,
    handle: InstanceHandle,
) -> Result<()> {
    let (result, requests) = with_stub(
        backend_fixture,
        vec![attached_sbs_instance(), patched()],
        async |backend| backend.detach_volume(&handle, "vol-cache").await,
    )
    .await?;
    result?;
    let patch = requests.get(1).context("detachment patch missing")?;
    assert_eq!(
        request_body(patch)?,
        serde_json::json!({
            "volumes": {
                "0": {
                    "id": "vol-root",
                    "boot": true,
                    "volume_type": "sbs_volume",
                }
            }
        })
    );
    Ok(())
}

/// The instance fixture with the root volume reported as Block Storage.
fn attached_sbs_instance() -> Reply {
    let mut instance: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/detach-instance.json"))
            .expect("detach fixture is JSON");
    let entry = instance
        .pointer_mut("/server/volumes/0")
        .expect("detach fixture has a root volume");
    entry["volume_type"] = serde_json::Value::String(String::from("sbs_volume"));
    Reply {
        status: 200,
        body: serde_json::to_string(&instance).expect("serialize fixture"),
    }
}
