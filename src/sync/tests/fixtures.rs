//! Shared fixtures for sync module tests.
//!
//! These fixtures are used across multiple sync test modules. Keeping them in
//! one place avoids duplication and ensures the test suite stays consistent.

use super::super::*;
use crate::backend::InstanceNetworking;
use rstest::fixture;
use std::net::{IpAddr, Ipv4Addr};

#[fixture]
pub fn base_config() -> SyncConfig {
    SyncConfig {
        rsync_bin: String::from("rsync"),
        ssh_bin: String::from("ssh"),
        ssh_user: String::from("ubuntu"),
        remote_path: String::from("/remote/path"),
        ssh_batch_mode: true,
        ssh_strict_host_key_checking: false,
        ssh_known_hosts_file: String::from("/dev/null"),
        ssh_identity_file: Some(String::from("~/.ssh/id_ed25519")),
        volume_mount_path: String::from("/mriya"),
        route_build_caches: true,
        create_cache_directories: true,
    }
}

#[fixture]
pub fn networking() -> InstanceNetworking {
    InstanceNetworking {
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        ssh_port: 2222,
    }
}
