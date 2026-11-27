//! Shell scripts that identify, format, mount and unmount the cache device.
//!
//! Scaleway exposes a Block Storage volume as a SCSI disk whose vendor is
//! `SCW`, whose model is `sbs`, and whose serial is `volume-<uuid>`. The
//! scripts resolve that serial rather than assuming `/dev/vdb`, refuse to touch
//! the root disk or any of its ancestors, refuse to format a device that
//! already carries a filesystem or partition-table signature, and verify the
//! mount source before reporting success. Every interpolated value is
//! shell-escaped.
//!
//! Each script prints a path line and exactly one machine-readable status
//! line:
//!
//! ```text
//! MRIYA_CACHE_PATH=/mriya
//! MRIYA_CACHE_STATUS=<status> DEVICE=/dev/sdb PATH=/mriya
//! ```
//!
//! The path has its own line because it may contain spaces, which the
//! `KEY=value` fields on the status line cannot carry.
//!
//! Domain refusals exit 0 so the orchestrator reports the parsed status rather
//! than a generic non-zero exit.

use shell_escape::unix::escape;

use super::prelude::{
    mount_source_helpers,
    probe_helpers,
    resolver_prelude,
    shared_prelude,
    wipefs_probe,
};

/// Prefix of the single machine-readable status line.
pub const STATUS_PREFIX: &str = "MRIYA_CACHE_STATUS=";
/// Prefix of the line carrying the mount path, which may contain spaces.
pub const PATH_PREFIX: &str = "MRIYA_CACHE_PATH=";
/// Filesystem label applied by the formatter.
pub const CACHE_LABEL: &str = "mriya-cache";

/// Escapes one value for safe interpolation into a single-quoted shell word.
pub(super) fn quoted(value: &str) -> String { escape(value.into()).into_owned() }

/// Builds the script that formats the new cache volume.
///
/// The script resolves the serial, guards the root disk, refuses a device with
/// an existing filesystem or partition-table signature, formats ext4 with the
/// `mriya-cache` label, flushes with `sync`, and reports `formatted`.
///
/// # Parameters
///
/// - `volume_id`: provider volume UUID whose guest serial is `volume-<uuid>`.
///
/// # Returns
///
/// Shell text. The caller must run it on the guest and parse the status line
/// from its output.
///
/// # Examples
///
/// ```
/// use mriya::cache_device::format_script;
///
/// let script = format_script("11111111-1111-1111-1111-111111111111");
/// assert!(script.contains("volume-11111111-1111-1111-1111-111111111111"));
/// assert!(script.contains("mkfs.ext4 -F -L mriya-cache"));
/// assert!(script.contains("wipefs --no-act"));
/// ```
#[must_use]
pub fn format_script(volume_id: &str) -> String {
    let prelude = format!("{}{}", shared_prelude(volume_id), wipefs_probe());
    format!(
        concat!(
            "{prelude}",
            "if ! sudo mkfs.ext4 -F -L {label} \"$mriya_device\" >/dev/null 2>&1; then\n",
            "  echo \"{prefix}format-failed DEVICE=$mriya_device\"\n",
            "  exit 0\n",
            "fi\n",
            "sync\n",
            "echo \"{prefix}formatted DEVICE=$mriya_device\"\n",
        ),
        prelude = prelude,
        prefix = STATUS_PREFIX,
        label = CACHE_LABEL,
    )
}

/// Builds the script that mounts the cache volume at `mount_path`.
///
/// The script requires an ext4 signature on a device with no partition table,
/// refuses a different source already mounted at the path, mounts the resolved
/// device, verifies with `findmnt` that the path's source is that device, and
/// gives the mount point to `owner`.
///
/// # Parameters
///
/// - `volume_id`: provider volume UUID whose guest serial is `volume-<uuid>`.
/// - `mount_path`: guest directory the volume is mounted at.
/// - `owner`: guest user that receives the mount point.
///
/// # Returns
///
/// Shell text. The caller must run it on the guest and parse the status line
/// from its output.
///
/// # Examples
///
/// ```
/// use mriya::cache_device::mount_script;
///
/// let script = mount_script("11111111-1111-1111-1111-111111111111", "/mriya", "ubuntu");
/// assert!(script.contains("findmnt -no SOURCE --mountpoint"));
/// assert!(script.contains("sudo chown \"$mriya_owner\" \"$mriya_path\""));
/// ```
#[must_use]
pub fn mount_script(volume_id: &str, mount_path: &str, owner: &str) -> String {
    let prelude = format!(
        "{}{}{}",
        shared_prelude(volume_id),
        probe_helpers(),
        mount_source_helpers()
    );
    format!(
        concat!(
            "{prelude}",
            "mriya_path={path}\n",
            "printf 'MRIYA_CACHE_PATH=%s\\n' \"$mriya_path\"\n",
            "mriya_owner={owner}\n",
            "mriya_probe TYPE\n",
            "mriya_type=$mriya_probe_value\n",
            "mriya_type_code=$mriya_probe_code\n",
            "mriya_probe PTTYPE\n",
            "mriya_pttype=$mriya_probe_value\n",
            "mriya_pttype_code=$mriya_probe_code\n",
            "if mriya_probe_failed \"$mriya_type_code\" || mriya_probe_failed \"$mriya_pttype_code\"; then\n",
            "  echo \"{prefix}probe-failed DEVICE=$mriya_device TYPE_CODE=$mriya_type_code PTTYPE_CODE=$mriya_pttype_code\"\n",
            "  exit 0\n",
            "fi\n",
            // `-s PTTYPE` filters the printed tags; it does not change the exit
            // status, so an ext4 disk without a partition table still exits 0
            // with an empty value. Only a non-empty value is a partition table.
            "if [ \"$mriya_type_code\" -ne 0 ] || [ \"$mriya_type\" != \"ext4\" ] || [ -n \"$mriya_pttype\" ]; then\n",
            "  echo \"{prefix}not-ext4 DEVICE=$mriya_device FSTYPE=$mriya_type PTTYPE=$mriya_pttype\"\n",
            "  exit 0\n",
            "fi\n",
            "mriya_mount_source \"$mriya_path\"\n",
            "if mriya_source_failed; then\n",
            "  echo \"{prefix}query-failed PATH=$mriya_path CODE=$mriya_source_code\"\n",
            "  exit 0\n",
            "fi\n",
            "if [ -n \"$mriya_source\" ] && [ \"$mriya_source\" != \"$mriya_device\" ]; then\n",
            "  echo \"{prefix}mount-occupied PATH=$mriya_path SOURCE=$mriya_source\"\n",
            "  exit 0\n",
            "fi\n",
            "if [ -z \"$mriya_source\" ]; then\n",
            // A failed query with no source could still be an occupied path;
            // mounting over it would hide the existing filesystem.
            "  if ! mriya_not_mounted \"$mriya_path\"; then\n",
            "    echo \"{prefix}query-failed PATH=$mriya_path CODE=$mriya_source_code\"\n",
            "    exit 0\n",
            "  fi\n",
            "  if ! sudo mkdir -p \"$mriya_path\" >/dev/null 2>&1; then\n",
            "    echo \"{prefix}mount-failed PATH=$mriya_path DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "  fi\n",
            "  if ! sudo mount -o defaults \"$mriya_device\" \"$mriya_path\" >/dev/null 2>&1; then\n",
            "    echo \"{prefix}mount-failed PATH=$mriya_path DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "  fi\n",
            "fi\n",
            "mriya_mount_source \"$mriya_path\"\n",
            "if [ \"$mriya_source\" != \"$mriya_device\" ]; then\n",
            "  echo \"{prefix}wrong-source PATH=$mriya_path EXPECTED=$mriya_device ACTUAL=$mriya_source\"\n",
            "  exit 0\n",
            "fi\n",
            "if ! sudo chown \"$mriya_owner\" \"$mriya_path\" >/dev/null 2>&1; then\n",
            "  echo \"{prefix}ownership-failed PATH=$mriya_path OWNER=$mriya_owner\"\n",
            "  exit 0\n",
            "fi\n",
            "echo \"{prefix}verified DEVICE=$mriya_device PATH=$mriya_path\"\n",
        ),
        prelude = prelude,
        path = quoted(mount_path),
        owner = quoted(owner),
        prefix = STATUS_PREFIX,
    )
}

/// Builds the script that flushes and unmounts the cache path.
///
/// The script refuses to unmount a path occupied by any source other than the
/// configured cache device, so a mount refusal reported by the mount script is
/// not followed by unmounting a foreign filesystem.
///
/// # Parameters
///
/// - `volume_id`: provider volume UUID whose guest serial is `volume-<uuid>`.
/// - `mount_path`: guest directory to unmount.
///
/// # Returns
///
/// Shell text. The caller must run it on the guest and parse the status line
/// from its output.
///
/// # Examples
///
/// ```
/// use mriya::cache_device::unmount_script;
///
/// let script = unmount_script("11111111-1111-1111-1111-111111111111", "/mriya");
/// assert!(script.contains("umount"));
/// assert!(script.contains("EXPECTED=$mriya_device"));
/// ```
#[must_use]
pub fn unmount_script(volume_id: &str, mount_path: &str) -> String {
    format!(
        concat!(
            "mriya_path={path}\n",
            "printf 'MRIYA_CACHE_PATH=%s\\n' \"$mriya_path\"\n",
            "{source}",
            "mriya_mount_source \"$mriya_path\"\n",
            "if mriya_source_failed; then\n",
            "  echo \"{prefix}query-failed PATH=$mriya_path CODE=$mriya_source_code\"\n",
            "  exit 0\n",
            "fi\n",
            "if [ -z \"$mriya_source\" ]; then\n",
            // A path that is still a mount point but whose source could not be
            // resolved must not be reported as unmounted: detaching the volume
            // under a live filesystem loses unflushed cache writes.
            "  if mriya_not_mounted \"$mriya_path\"; then\n",
            "    echo \"{prefix}unmounted PATH=$mriya_path\"\n",
            "    exit 0\n",
            "  fi\n",
            "  echo \"{prefix}query-failed PATH=$mriya_path CODE=unknown\"\n",
            "  exit 0\n",
            "fi\n",
            "{resolver}",
            // The resolver waits for udev and retries, so the source recorded
            // before it may be stale.
            "mriya_mount_source \"$mriya_path\"\n",
            "if mriya_source_failed; then\n",
            "  echo \"{prefix}query-failed PATH=$mriya_path CODE=$mriya_source_code\"\n",
            "  exit 0\n",
            "fi\n",
            "if [ \"$mriya_source\" != \"$mriya_device\" ]; then\n",
            "  echo \"{prefix}wrong-source PATH=$mriya_path EXPECTED=$mriya_device ACTUAL=$mriya_source\"\n",
            "  exit 0\n",
            "fi\n",
            "sync\n",
            "if sudo umount \"$mriya_path\" >/dev/null 2>&1; then\n",
            "  echo \"{prefix}unmounted PATH=$mriya_path\"\n",
            "else\n",
            "  echo \"{prefix}unmount-failed PATH=$mriya_path SOURCE=$mriya_source\"\n",
            "fi\n",
        ),
        path = quoted(mount_path),
        source = mount_source_helpers(),
        resolver = resolver_prelude(volume_id),
        prefix = STATUS_PREFIX,
    )
}
