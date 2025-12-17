//! Sync configuration validation and destination construction tests.

use super::super::*;
use crate::backend::InstanceNetworking;
use crate::test_helpers::EnvGuard;
use rstest::rstest;
use std::net::Ipv4Addr;

use super::fixtures::{base_config, networking};

/// Helper to assert validation rejects empty or whitespace values for a given field.
fn assert_validation_rejects_field<F>(mut cfg: SyncConfig, field_name: &str, set_field: F)
where
    F: Fn(&mut SyncConfig, String),
{
    for invalid in ["", "  "] {
        set_field(&mut cfg, invalid.to_owned());
        let Err(err) = cfg.validate() else {
            panic!("{field_name} '{invalid}' should fail");
        };
        let SyncError::InvalidConfig { ref field } = err else {
            panic!("expected InvalidConfig for {field_name}, got {err:?}");
        };
        assert_eq!(field, field_name, "expected invalid field {field_name}");
    }
}

/// Helper to assert that SSH identity validation fails with the expected error.
fn assert_ssh_identity_validation_fails(
    base_config: SyncConfig,
    ssh_identity_file: Option<String>,
) {
    let cfg = SyncConfig {
        ssh_identity_file,
        ..base_config
    };
    let err = cfg
        .validate()
        .expect_err("ssh_identity_file validation should fail");
    let SyncError::InvalidConfig { ref field } = err else {
        panic!("expected InvalidConfig, got {err:?}");
    };
    assert_eq!(field, "ssh_identity_file");
}

#[rstest]
fn sync_config_validate_accepts_defaults(base_config: SyncConfig) {
    assert!(base_config.validate().is_ok());
}

#[tokio::test]
async fn volume_mount_path_defaults_to_ortho_config_constant() {
    // Set SSH identity to satisfy validation; volume_mount_path should still use the default.
    let _guard = EnvGuard::set_vars(&[("MRIYA_SYNC_SSH_IDENTITY_FILE", "~/.ssh/id_ed25519")]).await;

    let sync_config = SyncConfig::load_without_cli_args()
        .expect("SyncConfig should load with defaults and env overrides");

    assert_eq!(
        sync_config.volume_mount_path, DEFAULT_VOLUME_MOUNT_PATH,
        "volume_mount_path should default to DEFAULT_VOLUME_MOUNT_PATH when omitted"
    );
}

#[rstest]
fn sync_config_validation_rejects_rsync_bin(base_config: SyncConfig) {
    assert_validation_rejects_field(base_config, "rsync_bin", |cfg, val| cfg.rsync_bin = val);
}

#[rstest]
fn sync_config_validation_rejects_ssh_bin(base_config: SyncConfig) {
    assert_validation_rejects_field(base_config, "ssh_bin", |cfg, val| cfg.ssh_bin = val);
}

#[rstest]
fn sync_config_validation_rejects_ssh_user(base_config: SyncConfig) {
    assert_validation_rejects_field(base_config, "ssh_user", |cfg, val| cfg.ssh_user = val);
}

#[rstest]
fn sync_config_validation_rejects_remote_path(base_config: SyncConfig) {
    assert_validation_rejects_field(base_config, "remote_path", |cfg, val| cfg.remote_path = val);
}

#[rstest]
fn sync_config_validation_rejects_missing_ssh_identity(base_config: SyncConfig) {
    assert_ssh_identity_validation_fails(base_config, None);
}

#[rstest]
fn sync_config_validation_rejects_empty_ssh_identity(base_config: SyncConfig) {
    assert_ssh_identity_validation_fails(base_config, Some(String::from("  ")));
}

#[rstest]
fn sync_error_invalid_config_produces_actionable_message(base_config: SyncConfig) {
    let cfg = SyncConfig {
        ssh_identity_file: None,
        ..base_config
    };
    let err = cfg
        .validate()
        .expect_err("missing ssh_identity_file should fail");
    let message = err.to_string();
    assert!(
        message.contains("MRIYA_SYNC_SSH_IDENTITY_FILE"),
        "error should mention env var: {message}"
    );
    assert!(
        message.contains("mriya.toml"),
        "error should mention config file: {message}"
    );
}

#[rstest]
fn remote_destination_builds_expected_values(
    base_config: SyncConfig,
    networking: InstanceNetworking,
) {
    let cfg = SyncConfig {
        ssh_user: String::from("alice"),
        remote_path: String::from("/dst"),
        ..base_config
    };
    let dest = cfg.remote_destination(&networking);
    let SyncDestination::Remote {
        user,
        host,
        port,
        path,
    } = dest
    else {
        panic!("expected remote destination");
    };
    assert_eq!(user, "alice");
    assert_eq!(host, Ipv4Addr::LOCALHOST.to_string());
    assert_eq!(port, 2222);
    assert_eq!(path, Utf8PathBuf::from("/dst"));
}
