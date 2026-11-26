//! Shell scripts that identify, format, mount and unmount the cache device.
//!
//! Scaleway exposes a Block Storage volume as a SCSI disk whose vendor is
//! `SCW`, whose model is `sbs`, and whose serial is `volume-<uuid>`. The
//! scripts resolve that serial rather than assuming `/dev/vdb`, refuse to touch
//! the root disk or any of its ancestors, refuse to format a device that
//! already carries a filesystem signature, and verify the mount source before
//! reporting success. Every interpolated value is shell-escaped.
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

/// Builds the resolver and root guard shared by the format and mount scripts.
///
/// The resolver reads `lsblk --scsi -P`, which quotes each value, and keeps a
/// disk only when vendor, model and serial all match. The guard resolves the
/// root mount source to its parent disk and refuses the candidate when it is
/// that disk, the root device itself, or an ancestor of either.
fn shared_prelude(volume_id: &str) -> String {
    let serial_name = format!("volume-{}", volume_id.trim());
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
            "mriya_root_source=$(findmnt -no SOURCE / 2>/dev/null || true)\n",
            "case \"$mriya_root_source\" in\n",
            "  /dev/*) ;;\n",
            "  *)\n",
            "    echo \"{prefix}root-unknown DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "    ;;\n",
            "esac\n",
            "mriya_root_name=${{mriya_root_source##*/}}\n",
            "mriya_root_disk=$(lsblk -no PKNAME \"/dev/$mriya_root_name\" 2>/dev/null)\n",
            "mriya_root_disk=${{mriya_root_disk%% *}}\n",
            "if [ -z \"$mriya_root_disk\" ]; then mriya_root_disk=\"$mriya_root_name\"; fi\n",
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
        serial = serial,
        attempts = RESOLVE_ATTEMPTS,
        prefix = STATUS_PREFIX,
    )
}

/// Builds the script that formats the new cache volume.
///
/// The script resolves the serial, guards the root disk, refuses a device with
/// an existing signature, formats ext4 with the `mriya-cache` label, flushes
/// with `sync`, and reports `formatted`.
///
/// # Examples
///
/// ```
/// use mriya::cache_device::format_script;
///
/// let script = format_script("11111111-1111-1111-1111-111111111111");
/// assert!(script.contains("volume-11111111-1111-1111-1111-111111111111"));
/// assert!(script.contains("mkfs.ext4 -F -L mriya-cache"));
/// ```
#[must_use]
pub fn format_script(volume_id: &str) -> String {
    format!(
        concat!(
            "{prelude}",
            "mriya_signature=$(sudo blkid -p -o value -s TYPE \"$mriya_device\" 2>/dev/null || true)\n",
            "if [ -n \"$mriya_signature\" ]; then\n",
            "  echo \"{prefix}signature-present DEVICE=$mriya_device FSTYPE=$mriya_signature\"\n",
            "  exit 0\n",
            "fi\n",
            "if ! sudo mkfs.ext4 -F -L {label} \"$mriya_device\" >/dev/null 2>&1; then\n",
            "  echo \"{prefix}format-failed DEVICE=$mriya_device\"\n",
            "  exit 0\n",
            "fi\n",
            "sync\n",
            "echo \"{prefix}formatted DEVICE=$mriya_device\"\n",
        ),
        prelude = shared_prelude(volume_id),
        prefix = STATUS_PREFIX,
        label = CACHE_LABEL,
    )
}

/// Builds the script that mounts the cache volume at `mount_path`.
///
/// The script requires an ext4 signature, refuses a different source already
/// mounted at the path, mounts the resolved device, verifies with `findmnt`
/// that the path's source is that device, and gives the mount point to
/// `owner`.
///
/// # Examples
///
/// ```
/// use mriya::cache_device::mount_script;
///
/// let script = mount_script("11111111-1111-1111-1111-111111111111", "/mriya", "ubuntu");
/// assert!(script.contains("findmnt -no SOURCE --mountpoint"));
/// assert!(script.contains(r#"chown "ubuntu":"ubuntu""#));
/// ```
#[must_use]
pub fn mount_script(volume_id: &str, mount_path: &str, owner: &str) -> String {
    format!(
        concat!(
            "{prelude}",
            "mriya_path={path}\n",
            "mriya_owner={owner}\n",
            "mriya_signature=$(sudo blkid -p -o value -s TYPE \"$mriya_device\" 2>/dev/null || true)\n",
            "if [ \"$mriya_signature\" != \"ext4\" ]; then\n",
            "  echo \"{prefix}not-ext4 DEVICE=$mriya_device FSTYPE=$mriya_signature\"\n",
            "  exit 0\n",
            "fi\n",
            "mriya_source=$(findmnt -no SOURCE --mountpoint \"$mriya_path\" 2>/dev/null || true)\n",
            "if [ -n \"$mriya_source\" ] && [ \"$mriya_source\" != \"$mriya_device\" ]; then\n",
            "  echo \"{prefix}mount-occupied PATH=$mriya_path SOURCE=$mriya_source\"\n",
            "  exit 0\n",
            "fi\n",
            "if [ -z \"$mriya_source\" ]; then\n",
            "  if ! sudo mkdir -p \"$mriya_path\" >/dev/null 2>&1; then\n",
            "    echo \"{prefix}mount-failed PATH=$mriya_path DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "  fi\n",
            "  if ! sudo mount -o defaults \"$mriya_device\" \"$mriya_path\" >/dev/null 2>&1; then\n",
            "    echo \"{prefix}mount-failed PATH=$mriya_path DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "  fi\n",
            "fi\n",
            "mriya_source=$(findmnt -no SOURCE --mountpoint \"$mriya_path\" 2>/dev/null || true)\n",
            "if [ \"$mriya_source\" != \"$mriya_device\" ]; then\n",
            "  echo \"{prefix}wrong-source PATH=$mriya_path EXPECTED=$mriya_device ACTUAL=$mriya_source\"\n",
            "  exit 0\n",
            "fi\n",
            "sudo chown \"$mriya_owner\":\"$mriya_owner\" \"$mriya_path\" >/dev/null 2>&1 || true\n",
            "echo \"{prefix}verified DEVICE=$mriya_device PATH=$mriya_path\"\n",
        ),
        prelude = shared_prelude(volume_id),
        path = quoted(mount_path),
        owner = quoted(owner),
        prefix = STATUS_PREFIX,
    )
}

/// Builds the script that flushes and unmounts the cache path.
///
/// # Examples
///
/// ```
/// use mriya::cache_device::unmount_script;
///
/// let script = unmount_script("/mriya");
/// assert!(script.contains("umount"));
/// ```
#[must_use]
pub fn unmount_script(mount_path: &str) -> String {
    format!(
        concat!(
            "mriya_path={path}\n",
            "mriya_source=$(findmnt -no SOURCE --mountpoint \"$mriya_path\" 2>/dev/null || true)\n",
            "if [ -z \"$mriya_source\" ]; then\n",
            "  echo \"{prefix}unmounted PATH=$mriya_path\"\n",
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
        prefix = STATUS_PREFIX,
    )
}
