//! Validation tests for `SyncConfig` and related error paths.
//!
//! Verifies that invalid configuration values are rejected with appropriate
//! error variants.

use std::net::{IpAddr, Ipv4Addr};

use super::test_doubles::ScriptedRunner;
use mriya::sync::{SyncConfig, SyncError, Syncer};

/// Helper to test that invalid values for a config field cause validation to fail.
fn assert_validation_rejects_field<F>(field_name: &str, set_field: F)
where
    F: Fn(&mut SyncConfig, String),
{
    for invalid in ["", "  "] {
        let mut cfg = base_config();
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

fn base_config() -> SyncConfig {
    SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: String::from("/remote"),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
    }
}

#[test]
fn sync_config_validation_rejects_rsync_bin() {
    assert_validation_rejects_field("rsync_bin", |cfg, val| cfg.rsync_bin = val);
}

#[test]
fn sync_config_validation_rejects_ssh_bin() {
    assert_validation_rejects_field("ssh_bin", |cfg, val| cfg.ssh_bin = val);
}

#[test]
fn sync_config_validation_rejects_ssh_user() {
    assert_validation_rejects_field("ssh_user", |cfg, val| cfg.ssh_user = val);
}

#[test]
fn sync_config_validation_rejects_remote_path() {
    assert_validation_rejects_field("remote_path", |cfg, val| cfg.remote_path = val);
}

#[test]
fn run_remote_reports_missing_exit_code() {
    let runner = ScriptedRunner::new();
    runner.push_missing_exit_code();

    let syncer = Syncer::new(base_config(), runner).expect("config should be valid");
    let networking = mriya::InstanceNetworking {
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        ssh_port: 22,
    };

    let output = syncer
        .run_remote(&networking, "echo ok")
        .expect("missing exit code should now be propagated");

    assert!(output.exit_code.is_none());
}
