//! Remote command wrapping, including optional cache routing.
//!
//! The sync module needs to run remote commands in a predictable working
//! directory and, when a persistent cache volume is mounted, route build caches
//! into that volume. This module centralizes the string-building logic so the
//! top-level sync module remains focused on orchestration.

use shell_escape::unix::escape;

use super::SyncConfig;

/// Subdirectories created under the cache volume mount point.
///
/// These paths correspond to the environment variables exported by the cache
/// routing preamble. Creating them eagerly after mount ensures language
/// toolchains can write immediately without failing due to missing
/// directories.
pub const CACHE_SUBDIRECTORIES: &[&str] = &[
    "cargo",
    "rustup",
    "target",
    "go/pkg/mod",
    "go/build-cache",
    "pip/cache",
    "npm/cache",
    "yarn/cache",
    "pnpm/store",
];

/// How a remote command treats the cache volume mount.
///
/// The two states are the only ones that can reach a remote command: a run
/// without a configured cache, and a run whose configured cache was verified
/// before the command started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CacheRouting {
    /// No cache volume is configured; keep the guarded historical preamble.
    ///
    /// Exports are applied only when something is mounted at the cache path,
    /// which preserves the behaviour of runs that never configured a volume.
    Unconfigured,
    /// A verified cache volume must still back the mount path.
    ///
    /// The preamble stops before the user command when the path's source is
    /// not the verified device, so a lost or replaced mount cannot silently
    /// redirect build output onto the root filesystem.
    Required {
        /// Guest device path reported when the mount was verified.
        device: String,
    },
}

/// Builds a remote command string with an optional cache routing preamble.
///
/// The remote path is shell-escaped, cache exports are prepended when routing
/// is enabled, and the user command is wrapped with a directory change. For a
/// configured cache the preamble fails closed; for an unconfigured run the
/// historical `mountpoint -q` guard is kept.
pub(crate) fn build_remote_command(
    config: &SyncConfig,
    remote_command: &str,
    routing: &CacheRouting,
) -> String {
    let escaped_path = escape(config.remote_path.as_str().into());
    let cache_preamble = cache_routing_preamble(config, routing);
    if cache_preamble.is_empty() {
        format!("cd {escaped_path} && {remote_command}")
    } else {
        format!("{cache_preamble}cd {escaped_path} && {remote_command}")
    }
}

/// Builds a shell prefix that exports build cache locations under the
/// volume mount path.
///
/// Returns an empty string when cache routing is disabled and no cache is
/// required, so a run that opts out never touches the cache paths. A required
/// cache keeps its source check even when the exports are disabled.
fn cache_routing_preamble(config: &SyncConfig, routing: &CacheRouting) -> String {
    let Some(guard) = mount_guard(config, routing) else {
        return String::new();
    };
    let mut preamble = guard;
    if config.route_build_caches {
        preamble.push_str(&cache_exports(config));
    }
    if matches!(routing, CacheRouting::Unconfigured) {
        preamble.push_str("fi; ");
    }
    preamble
}

/// Returns the guard that opens the preamble, or `None` when none is needed.
fn mount_guard(config: &SyncConfig, routing: &CacheRouting) -> Option<String> {
    let mount_path = &config.volume_mount_path;
    let escaped_mount = escape(mount_path.as_str().into());
    match routing {
        // A run without a configured cache has nothing to guard when routing
        // is disabled, but a required cache must still be verified.
        CacheRouting::Unconfigured if !config.route_build_caches => None,
        CacheRouting::Unconfigured => Some(format!(
            "if mountpoint -q {escaped_mount} 2>/dev/null; then "
        )),
        CacheRouting::Required { device } => {
            let escaped_device = escape(device.as_str().into());
            let message = escape(
                format!(
                    "mriya: cache volume is not mounted at {mount_path} from {device}; refusing to run"
                )
                .into(),
            );
            Some(format!(
                concat!(
                    "if [ \"$(findmnt -no SOURCE --mountpoint {mount} 2>/dev/null)\" != {device} ]; then ",
                    "printf '%s\\n' {message} >&2; exit 1; fi; "
                ),
                mount = escaped_mount,
                device = escaped_device,
                message = message,
            ))
        }
    }
}

/// Returns the export statements for every cache variable.
fn cache_exports(config: &SyncConfig) -> String {
    let mount_path = &config.volume_mount_path;
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
    for (key, value) in exports {
        let escaped_value = escape(value.into());
        preamble.push_str("export ");
        preamble.push_str(key);
        preamble.push('=');
        preamble.push_str(escaped_value.as_ref());
        preamble.push_str("; ");
    }
    preamble
}

/// Builds a shell command that creates all cache subdirectories under the
/// given mount path.
///
/// The command uses `sudo mkdir -p` to create the directory tree idempotently.
/// Sudo is required because the mount point is typically root-owned after
/// mounting, while the SSH user may be non-root (e.g., "ubuntu").
/// Fails silently if the directories cannot be created (the volume may be
/// read-only or the mount may have failed).
///
/// # Example
///
/// ```
/// use mriya::sync::create_cache_directories_command;
///
/// let cmd = create_cache_directories_command("/mriya");
/// assert!(cmd.contains("sudo mkdir -p"));
/// assert!(cmd.contains("/mriya/cargo"));
/// ```
#[must_use]
pub fn create_cache_directories_command(mount_path: &str) -> String {
    let paths: Vec<String> = CACHE_SUBDIRECTORIES
        .iter()
        .map(|sub| {
            let full_path = format!("{mount_path}/{sub}");
            escape(full_path.into()).into_owned()
        })
        .collect();
    format!("sudo mkdir -p {} 2>/dev/null || true", paths.join(" "))
}
