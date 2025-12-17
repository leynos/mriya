//! Remote command wrapping, including optional cache routing.
//!
//! The sync module needs to run remote commands in a predictable working
//! directory and, when a persistent cache volume is mounted, route build caches
//! into that volume. This module centralises the string-building logic so the
//! top-level sync module remains focused on orchestration.

use shell_escape::unix::escape;

use super::SyncConfig;

/// Builds a remote command string with an optional cache routing preamble.
///
/// The remote path is shell-escaped, cache exports are prepended when enabled,
/// and the user command is wrapped with a directory change.
pub(crate) fn build_remote_command(config: &SyncConfig, remote_command: &str) -> String {
    let escaped_path = escape(config.remote_path.as_str().into());
    let cache_preamble = cache_routing_preamble(config);
    if cache_preamble.is_empty() {
        format!("cd {escaped_path} && {remote_command}")
    } else {
        format!("{cache_preamble}cd {escaped_path} && {remote_command}")
    }
}

fn cache_routing_preamble(config: &SyncConfig) -> String {
    if !config.route_build_caches {
        return String::new();
    }

    let mount_path = &config.volume_mount_path;
    let escaped_mount = escape(mount_path.as_str().into());

    let exports = [
        ("CARGO_HOME", format!("{mount_path}/cargo")),
        ("RUSTUP_HOME", format!("{mount_path}/rustup")),
        ("CARGO_TARGET_DIR", format!("{mount_path}/target")),
        ("GOMODCACHE", format!("{mount_path}/go/pkg/mod")),
        ("GOCACHE", format!("{mount_path}/go/build-cache")),
        ("PIP_CACHE_DIR", format!("{mount_path}/pip/cache")),
        ("npm_config_cache", format!("{mount_path}/npm/cache")),
        ("YARN_CACHE_FOLDER", format!("{mount_path}/yarn/cache")),
        ("PNPM_STORE_PATH", format!("{mount_path}/pnpm/store")),
    ];

    let mut preamble = String::new();
    preamble.push_str("if mountpoint -q ");
    preamble.push_str(escaped_mount.as_ref());
    preamble.push_str(" 2>/dev/null; then ");
    for (key, value) in exports {
        let escaped_value = escape(value.into());
        preamble.push_str("export ");
        preamble.push_str(key);
        preamble.push('=');
        preamble.push_str(escaped_value.as_ref());
        preamble.push_str("; ");
    }
    preamble.push_str("fi; ");
    preamble
}
