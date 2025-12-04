use std::net::{IpAddr, Ipv4Addr};

use mriya::sync::{SyncConfig, SyncError, Syncer};
use super::test_doubles::ScriptedRunner;

/// Helper to test that invalid values for a config field cause validation to fail
fn assert_validation_rejects_field<F>(field_name: &str, set_field: F)
where
    F: Fn(&mut SyncConfig, String),
{
    for invalid in ["", "  "] {
        let mut cfg = SyncConfig {
            rsync_bin: String::from("rsync"),
            ssh_bin: String::from("ssh"),
            ssh_user: String::from("ubuntu"),
            remote_path: String::from("/remote"),
            ssh_batch_mode: true,
            ssh_strict_host_key_checking: false,
            ssh_known_hosts_file: String::from("/dev/null"),
        };
        set_field(&mut cfg, invalid.to_owned());
        let Err(err) = cfg.validate() else {
            panic!("{field_name} '{invalid}' should fail");
        };
        assert!(
            matches!(err, SyncError::InvalidConfig { field } if field == field_name),
            "expected InvalidConfig for {field_name}, got {err:?}"
        );
    }
}

#[test]
fn sync_config_validation_rejects_empty_remote_path() {
    let cfg = SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: String::new(),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
    };

    let err = cfg
        .validate()
        .expect_err("missing remote_path should be rejected");
    assert!(matches!(
        err,
        SyncError::InvalidConfig { field } if field == "remote_path"
    ));
}

#[test]
fn sync_config_validation_rejects_rsync_bin_values() {
    assert_validation_rejects_field("rsync_bin", |cfg, val| cfg.rsync_bin = val);
}

#[test]
fn sync_config_validation_rejects_ssh_bin_values() {
    assert_validation_rejects_field("ssh_bin", |cfg, val| cfg.ssh_bin = val);
}

#[test]
fn sync_config_validation_rejects_ssh_user_values() {
    assert_validation_rejects_field("ssh_user", |cfg, val| cfg.ssh_user = val);
}

#[test]
fn sync_config_validation_rejects_remote_path_values() {
    assert_validation_rejects_field("remote_path", |cfg, val| cfg.remote_path = val);
}

#[test]
fn run_remote_reports_missing_exit_code() {
    let runner = ScriptedRunner::new();
    runner.push_missing_exit_code();

    let config = SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: String::from("/remote"),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
    };

    let syncer = Syncer::new(config, runner).expect("config should be valid");
    let networking = mriya::InstanceNetworking {
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        ssh_port: 22,
    };

    let err = syncer
        .run_remote(&networking, "echo ok")
        .expect_err("missing exit code should error");

    assert!(matches!(
        err,
        SyncError::MissingExitCode { program } if program == "ssh"
    ));
}
