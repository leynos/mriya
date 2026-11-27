//! Shell preludes shared by the cache-device scripts.
//!
//! The resolver, root guard, probe helpers and mount-source queries are
//! built once here so the four public scripts cannot drift apart.

use super::script::{STATUS_PREFIX, quoted};

/// How many half-second attempts the resolver makes after `udevadm settle`.
const RESOLVE_ATTEMPTS: u32 = 20;

/// Builds the serial resolver shared by every script.
///
/// The resolver reads `lsblk --scsi -P`, which quotes each value, and keeps a
/// disk only when vendor, model and serial all match. It sets `mriya_device`,
/// or reports `not-found`/`ambiguous` and exits.
pub(super) fn resolver_prelude(volume_id: &str) -> String {
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
            "  mriya_scsi=$(lsblk --scsi -P -o NAME,VENDOR,MODEL,SERIAL,TYPE 2>/dev/null)\n",
            "  if [ \"$?\" -ne 0 ]; then\n",
            "    echo \"{prefix}probe-failed DEVICE=serial:$mriya_serial REASON=lsblk\"\n",
            "    exit 0\n",
            "  fi\n",
            "  mriya_matches=$(printf '%s\\n' \"$mriya_scsi\" | awk -v want=\"$mriya_serial\" '\n",
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
pub(super) fn root_guard() -> String {
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
            "mriya_root_status=$?\n",
            "if [ \"$mriya_root_status\" -ne 0 ]; then\n",
            "  echo \"{prefix}root-unknown DEVICE=$mriya_device\"\n",
            "  exit 0\n",
            "fi\n",
            "mriya_root_lines=$(printf '%s\\n' \"$mriya_root_disk\" | awk 'NF {{ n++ }} END {{ print n + 0 }}')\n",
            "if [ \"$mriya_root_lines\" -gt 1 ]; then\n",
            "  echo \"{prefix}root-unknown DEVICE=$mriya_device\"\n",
            "  exit 0\n",
            "fi\n",
            "mriya_root_disk=$(printf '%s\\n' \"$mriya_root_disk\" | awk 'NF {{ print; exit }}')\n",
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
            "  mriya_walk_status=$?\n",
            "  if [ \"$mriya_walk_status\" -ne 0 ]; then\n",
            "    echo \"{prefix}root-unknown DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "  fi\n",
            "  mriya_walk_lines=$(printf '%s\\n' \"$mriya_walk\" | awk 'NF {{ n++ }} END {{ print n + 0 }}')\n",
            "  if [ \"$mriya_walk_lines\" -gt 1 ]; then\n",
            "    echo \"{prefix}root-unknown DEVICE=$mriya_device\"\n",
            "    exit 0\n",
            "  fi\n",
            "  mriya_walk=$(printf '%s\\n' \"$mriya_walk\" | awk 'NF {{ print; exit }}')\n",
            "  mriya_depth=$((mriya_depth + 1))\n",
            "done\n",
        ),
        prefix = STATUS_PREFIX,
    )
}

/// Builds the resolver and root guard shared by the format and mount scripts.
pub(super) fn shared_prelude(volume_id: &str) -> String {
    format!("{}{}", resolver_prelude(volume_id), root_guard())
}

/// Builds the `blkid` probe used by the mount script.
///
/// `mriya_probe <tag>` leaves the value in `mriya_probe_value` and the exit
/// status in `mriya_probe_code`. Statuses 4 and 8 are ambiguous or failed
/// results, never an absent signature.
pub(super) fn probe_helpers() -> String {
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
    )
    .to_owned()
}

/// Builds the `wipefs` signature probe used before formatting.
///
/// `wipefs --no-act` lists every signature and exits non-zero when the device
/// cannot be probed, so the formatter can tell a clean, empty disk from a
/// failed probe; `blkid -p` uses exit status 2 for both.
pub(super) fn wipefs_probe() -> String {
    format!(
        concat!(
            "mriya_signatures=$(sudo wipefs --no-act \"$mriya_device\" 2>/dev/null)\n",
            "mriya_signatures_code=$?\n",
            "if [ \"$mriya_signatures_code\" -ne 0 ]; then\n",
            "  echo \"{prefix}probe-failed DEVICE=$mriya_device CODE=$mriya_signatures_code\"\n",
            "  exit 0\n",
            "fi\n",
            "if [ -n \"$mriya_signatures\" ]; then\n",
            "  echo \"{prefix}signature-present DEVICE=$mriya_device\"\n",
            "  exit 0\n",
            "fi\n",
        ),
        prefix = STATUS_PREFIX
    )
}

/// Builds the mount-source query helpers shared by the mount and unmount
/// scripts.
///
/// `findmnt --mountpoint` returns 1 when the path is not a mount point but
/// also when the query itself fails, so `mountpoint -q` is consulted before a
/// failed query is read as "nothing mounted".
pub(super) fn mount_source_helpers() -> String {
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
