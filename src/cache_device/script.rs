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
//! Each script prints exactly one machine-readable line:
//!
//! ```text
//! MRIYA_CACHE_STATUS=<status> DEVICE=/dev/sdb PATH=/mriya
//! ```
//!
//! Domain refusals exit 0 so the orchestrator reports the parsed status rather
//! than a generic non-zero exit.

use shell_escape::unix::escape;

/// Prefix of the single machine-readable status line.
pub const STATUS_PREFIX: &str = "MRIYA_CACHE_STATUS=";
/// Filesystem label applied by the formatter.
pub const CACHE_LABEL: &str = "mriya-cache";
/// How many half-second attempts the resolver makes after `udevadm settle`.
const RESOLVE_ATTEMPTS: u32 = 20;

/// Escapes one value for safe interpolation into a single-quoted shell word.
fn quoted(value: &str) -> String { escape(value.into()).into_owned() }

/// Builds the serial resolver shared by every script.
///
/// The resolver reads `lsblk --scsi -P`, which quotes each value, and keeps a
/// disk only when vendor, model and serial all match. It sets `mriya_device`,
/// or reports `not-found`/`ambiguous` and exits.
fn resolver_prelude(volume_id: &str) -> String {
    let serial_name = format!("volume-{}", volume_id.trim().to_ascii_lowercase());
    let serial = quoted(&serial_name);
    format!(
        concat!(
            "mriya_serial={serial}\n",
            "mriya_device=''\n",
            "mriya_matches=''\n",
            "udevadm settle --timeout=10 >/dev/null 2>&1 || true\n",
            "mriya_tries=0\n",
            "while [ \"$mriya_tries\" -lt {attempts} ]; do\n",
            "  mriya_matches=$(lsblk --scsi -P -o NAME,VENDOR,MODEL,SERIAL,TYPE 2>/dev/null | awk -v want=\"$mriya_serial\" '\n",
            "    {{\n",
            "      name=\"\"; vendor=\"\"; model=\"\"; serial=\"\"; type=\"\";\n",
            "      for (i = 1; i <= NF; i++) {{\n",
            "        split($i, pair, \"=\");\n",
            "        key = pair[1]; value = pair[2]; gsub(/\"/, \"\", value);\n",
            "        if (key == \"NAME\") name = value;\n",
            "        else if (key == \"VENDOR\") vendor = value;\n",
            "        else if (key == \"MODEL\") model = value;\n",
            "        else if (key == \"SERIAL\") serial = value;\n",
            "        else if (key == \"TYPE\") type = value;\n",
            "      }}\n",
            "      if (vendor == \"SCW\" && model == \"sbs\" && serial == want && type == \"disk\") print name;\n",
            "    }}')\n",
            "  mriya_count=$(printf '%s\\n' \"$mriya_matches\" | awk 'NF {{ n++ }} END {{ print n + 0 }}')\n",
            "  if [ \"$mriya_count\" -ge 1 ]; then break; fi\n",
            "  mriya_tries=$((mriya_tries + 1))\n",
            "  sleep 0.5\n",
            "done\n",
            "if [ \"$mriya_count\" -eq 0 ]; then\n",
            "  echo \"{prefix}not-found DEVICE=serial:$mriya_serial\"\n",
            "  exit 0\n",
            "fi\n",
            "if [ \"$mriya_count\" -gt 1 ]; then\n",
            "  echo \"{prefix}ambiguous DEVICE=serial:$mriya_serial\"\n",
            "  exit 0\n",
            "fi\n",
            "mriya_device=\"/dev/$mriya_matches\"\n",
        ),
        serial = serial,
        attempts = RESOLVE_ATTEMPTS,
        prefix = STATUS_PREFIX,
    )
}

/// Builds the root-disk guard shared by the format and mount scripts.
///
/// The guard resolves the root mount source to its parent disk. It passes the
/// full source path to `lsblk`, so a device-mapper source is resolved through
/// its parents; a source that cannot be resolved is refused rather than
/// guessed. The candidate is refused when it is that disk, the root device, or
/// an ancestor of either.
fn root_guard() -> String {
    format!(
        concat!(
            "mriya_root_source=$(findmnt -no SOURCE / 2>/dev/null || true)\n",
            "case \"$mriya_root_source\" in\n",
            "  /dev/*) ;;\n",
            "  *)\n",
            "    echo \"{prefix}root-unknown DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "    ;;\n",
            "esac\n",
            "mriya_root_disk=$(lsblk -no PKNAME \"$mriya_root_source\" 2>/dev/null)\n",
            "mriya_root_disk=${{mriya_root_disk%% *}}\n",
            "mriya_root_name=${{mriya_root_source##*/}}\n",
            "if [ -z \"$mriya_root_disk\" ]; then\n",
            "  case \"$mriya_root_source\" in\n",
            "    /dev/sd*|/dev/vd*|/dev/xvd*|/dev/nvme*n*) mriya_root_disk=\"$mriya_root_name\" ;;\n",
            "    *)\n",
            "      echo \"{prefix}root-unknown DEVICE=$mriya_device\"\n",
            "      exit 0\n",
            "      ;;\n",
            "  esac\n",
            "fi\n",
            "mriya_candidate=${{mriya_device##*/}}\n",
            "mriya_walk=\"$mriya_root_disk\"\n",
            "mriya_depth=0\n",
            "while [ -n \"$mriya_walk\" ] && [ \"$mriya_depth\" -lt 8 ]; do\n",
            "  if [ \"$mriya_candidate\" = \"$mriya_walk\" ] || [ \"$mriya_candidate\" = \"$mriya_root_name\" ]; then\n",
            "    echo \"{prefix}root-disk DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "  fi\n",
            "  mriya_walk=$(lsblk -no PKNAME \"/dev/$mriya_walk\" 2>/dev/null)\n",
            "  mriya_walk=${{mriya_walk%% *}}\n",
            "  mriya_depth=$((mriya_depth + 1))\n",
            "done\n",
        ),
        prefix = STATUS_PREFIX,
    )
}

/// Builds the resolver and root guard shared by the format and mount scripts.
fn shared_prelude(volume_id: &str) -> String {
    format!("{}{}", resolver_prelude(volume_id), root_guard())
}

/// Builds the `blkid` and readability probes used by format and mount.
///
/// `mriya_probe <tag>` leaves the value in `mriya_probe_value` and the exit
/// status in `mriya_probe_code`. `blkid -p` returns 2 both when a signature is
/// absent and when it cannot open or read the device, so a 2 alone proves
/// nothing: `mriya_readable` must also succeed before the device may be
/// treated as an empty disk.
fn probe_helpers() -> String {
    concat!(
        "mriya_probe() {\n",
        "  mriya_probe_value=$(sudo blkid -p -o value -s \"$1\" \"$mriya_device\" 2>/dev/null)\n",
        "  mriya_probe_code=$?\n",
        "}\n",
        "mriya_probe_failed() {\n",
        "  case \"$1\" in\n",
        "    4|8) return 0 ;;\n",
        "  esac\n",
        "  return 1\n",
        "}\n",
        "mriya_readable() {\n",
        "  sudo dd if=\"$mriya_device\" of=/dev/null bs=512 count=1 >/dev/null 2>&1\n",
        "}\n",
    )
    .to_owned()
}

/// Builds the mount-source query helpers shared by the mount and unmount
/// scripts.
///
/// `findmnt --mountpoint` returns 1 when the path is not a mount point but
/// also when the query itself fails, so `mountpoint -q` is consulted before a
/// failed query is read as "nothing mounted".
fn mount_source_helpers() -> String {
    concat!(
        "mriya_mount_source() {\n",
        "  mriya_source=$(findmnt -no SOURCE --mountpoint \"$1\" 2>/dev/null)\n",
        "  mriya_source_code=$?\n",
        "}\n",
        "mriya_source_failed() {\n",
        "  if [ \"$mriya_source_code\" -gt 1 ]; then return 0; fi\n",
        "  if [ \"$mriya_source_code\" -eq 0 ] && [ -z \"$mriya_source\" ]; then return 0; fi\n",
        "  return 1\n",
        "}\n",
        // `mountpoint -q` reports 32 for an existing path that is not a mount
        // point, and 1 for a missing path or a failed stat. Treat 1 as "not
        // mounted" only when the path is confirmed absent.
        "mriya_not_mounted() {\n",
        "  mountpoint -q \"$1\" 2>/dev/null\n",
        "  case \"$?\" in\n",
        "    32) return 0 ;;\n",
        "    1) [ ! -e \"$1\" ] && return 0 ;;\n",
        "  esac\n",
        "  return 1\n",
        "}\n",
    )
    .to_owned()
}

/// Builds the script that formats the new cache volume.
///
/// The script resolves the serial, guards the root disk, refuses a device with
/// an existing filesystem or partition-table signature, formats ext4 with the
/// `mriya-cache` label, flushes with `sync`, and reports `formatted`.
///
/// # Examples
///
/// ```
/// use mriya::cache_device::format_script;
///
/// let script = format_script("11111111-1111-1111-1111-111111111111");
/// assert!(script.contains("volume-11111111-1111-1111-1111-111111111111"));
/// assert!(script.contains("mkfs.ext4 -F -L mriya-cache"));
/// assert!(script.contains("PTTYPE"));
/// ```
#[must_use]
pub fn format_script(volume_id: &str) -> String {
    let prelude = format!("{}{}", shared_prelude(volume_id), probe_helpers());
    format!(
        concat!(
            "{prelude}",
            "mriya_probe TYPE\n",
            "mriya_type=$mriya_probe_value\n",
            "mriya_type_code=$mriya_probe_code\n",
            "mriya_probe UUID\n",
            "mriya_uuid=$mriya_probe_value\n",
            "mriya_uuid_code=$mriya_probe_code\n",
            "mriya_probe PTTYPE\n",
            "mriya_pttype=$mriya_probe_value\n",
            "mriya_pttype_code=$mriya_probe_code\n",
            "if mriya_probe_failed \"$mriya_type_code\" || mriya_probe_failed \"$mriya_uuid_code\" || mriya_probe_failed \"$mriya_pttype_code\"; then\n",
            "  echo \"{prefix}probe-failed DEVICE=$mriya_device TYPE_CODE=$mriya_type_code UUID_CODE=$mriya_uuid_code PTTYPE_CODE=$mriya_pttype_code\"\n",
            "  exit 0\n",
            "fi\n",
            // An empty probe only proves the disk is empty when the disk can
            // be read at all; `blkid` also returns 2 for an unreadable device.
            "if ! mriya_readable; then\n",
            "  echo \"{prefix}device-unreadable DEVICE=$mriya_device\"\n",
            "  exit 0\n",
            "fi\n",
            "if [ \"$mriya_type_code\" -ne 2 ] || [ \"$mriya_uuid_code\" -ne 2 ] || [ \"$mriya_pttype_code\" -ne 2 ]; then\n",
            "  echo \"{prefix}signature-present DEVICE=$mriya_device FSTYPE=$mriya_type PTTYPE=$mriya_pttype\"\n",
            "  exit 0\n",
            "fi\n",
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
/// # Examples
///
/// ```
/// use mriya::cache_device::mount_script;
///
/// let script = mount_script("11111111-1111-1111-1111-111111111111", "/mriya", "ubuntu");
/// assert!(script.contains("findmnt -no SOURCE --mountpoint"));
/// assert!(script.contains("sudo chown \"$mriya_owner\":\"$mriya_owner\""));
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
            "if ! sudo chown \"$mriya_owner\":\"$mriya_owner\" \"$mriya_path\" >/dev/null 2>&1; then\n",
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
