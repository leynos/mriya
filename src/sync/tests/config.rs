//! Sync configuration validation and destination construction tests.

use super::super::*;
use crate::backend::InstanceNetworking;
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

fn set_rsync_bin(cfg: &mut SyncConfig, val: String) {
    cfg.rsync_bin = val;
}

fn set_ssh_bin(cfg: &mut SyncConfig, val: String) {
    cfg.ssh_bin = val;
}

fn set_ssh_user(cfg: &mut SyncConfig, val: String) {
    cfg.ssh_user = val;
}

fn set_remote_path(cfg: &mut SyncConfig, val: String) {
    cfg.remote_path = val;
}

#[rstest]
fn sync_config_validate_accepts_defaults(base_config: SyncConfig) {
    base_config
        .validate()
        .expect("sync configuration should validate");
}

#[rstest]
fn sync_config_validate_accepts_none_ssh_identity(base_config: SyncConfig) {
    let cfg = SyncConfig {
        ssh_identity_file: None,
        ..base_config
    };
    cfg.validate()
        .expect("None ssh_identity_file should be valid (SSH uses defaults)");
}

/// Loads through injected sources so the assertion does not depend on the
/// process environment or a developer's real configuration files.
#[test]
fn volume_mount_path_uses_default_when_unset() {
    use ortho_config::{MapEnv, OrthoConfig as _, SharedEnvSource, SharedScanEnvSource};
    use std::sync::Arc;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let home = camino::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf())
        .expect("temp path should be utf8");
    // Selecting an absent path suppresses project-file discovery, so a
    // `mriya.toml` in the working directory cannot supply the value.
    let missing = home.join("missing.toml");
    let environment = Arc::new(
        MapEnv::new()
            .with_var("HOME", home.as_str())
            .with_var("XDG_CONFIG_HOME", home.as_str())
            .with_var("XDG_CONFIG_DIRS", home.as_str())
            .with_var("MRIYA_CONFIG_PATH", missing.as_str()),
    );
    let discovery: SharedEnvSource = environment.clone();
    let merge: SharedScanEnvSource = environment;
    let sync_config = SyncConfig::load_from_iter_with_sources(["mriya"], discovery, merge)
        .expect("SyncConfig should load with defaults");

    assert_eq!(
        sync_config.volume_mount_path, DEFAULT_VOLUME_MOUNT_PATH,
        "volume_mount_path should default to DEFAULT_VOLUME_MOUNT_PATH when omitted"
    );
}

#[rstest]
#[case("rsync_bin", set_rsync_bin)]
#[case("ssh_bin", set_ssh_bin)]
#[case("ssh_user", set_ssh_user)]
#[case("remote_path", set_remote_path)]
fn sync_config_validation_rejects_invalid_field(
    base_config: SyncConfig,
    #[case] field_name: &str,
    #[case] set_field: fn(&mut SyncConfig, String),
) {
    assert_validation_rejects_field(base_config, field_name, set_field);
}

#[rstest]
fn sync_config_validation_rejects_empty_ssh_identity(base_config: SyncConfig) {
    assert_ssh_identity_validation_fails(base_config.clone(), Some(String::new()));
    assert_ssh_identity_validation_fails(base_config, Some(String::from("  ")));
}

#[rstest]
fn sync_error_invalid_config_produces_actionable_message(base_config: SyncConfig) {
    let cfg = SyncConfig {
        ssh_identity_file: Some(String::new()),
        ..base_config
    };
    let err = cfg
        .validate()
        .expect_err("empty ssh_identity_file should fail");
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
