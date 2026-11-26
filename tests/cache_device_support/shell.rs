//! Fake guest utilities for the cache-device shell tests.
//!
//! Each fake logs its arguments to a shared file and reads its state from small
//! files under the sandbox root, so a case can model a filesystem, a partition
//! table, an ambiguous probe, a device-mapper root, a duplicate serial or a
//! failing command without touching the host.

/// Reads a state file, printing nothing when it is absent.
fn read_state(root: &str, name: &str) -> String {
    format!(
        concat!(
            "if [ -f '{root}/{name}' ]; then ",
            "while IFS= read -r line; do printf '%s\\n' \"$line\"; done < '{root}/{name}'; fi"
        ),
        root = root,
        name = name,
    )
}

/// Builds the resolver and root-guard `lsblk` fake.
fn fake_lsblk(root: &str) -> String {
    format!(
        concat!(
            "printf 'lsblk %s\\n' \"$*\" >> '{root}/tools.log'\n",
            "case \"$*\" in\n",
            "  *PKNAME*)\n",
            "    for arg in \"$@\"; do target=\"$arg\"; done\n",
            "    name=\"${{target##*/}}\"\n",
            "    file=\"{root}/pkname-$name\"\n",
            "    if [ -f \"$file\" ]; then while IFS= read -r line; do ",
            "printf '%s\\n' \"$line\"; done < \"$file\"; fi\n",
            "    ;;\n",
            "  *)\n",
            "    {scsi}\n",
            "    ;;\n",
            "esac"
        ),
        root = root,
        scsi = read_state(root, "scsi"),
    )
}

/// Builds the `findmnt` fake for the root source and mount-point queries.
fn fake_findmnt(root: &str) -> String {
    format!(
        concat!(
            "printf 'findmnt %s\\n' \"$*\" >> '{root}/tools.log'\n",
            "case \"$*\" in\n",
            "  *--mountpoint*)\n",
            "    {mount}\n",
            "    ;;\n",
            "  *)\n",
            "    {root_source}\n",
            "    ;;\n",
            "esac"
        ),
        root = root,
        mount = read_state(root, "mount-source"),
        root_source = read_state(root, "root-source"),
    )
}

/// Builds the `blkid` fake, which reports the configured signature.
///
/// The fake distinguishes the `TYPE` and `PTTYPE` probes so a case can model a
/// filesystem, a partition table, an ambiguous low-level result or an
/// operational failure independently.
fn fake_blkid(root: &str) -> String {
    format!(
        concat!(
            "printf 'blkid %s\\n' \"$*\" >> '{root}/tools.log'\n",
            "tag='TYPE'\n",
            "prev=''\n",
            "for arg in \"$@\"; do\n",
            "  if [ \"$prev\" = '-s' ]; then tag=\"$arg\"; fi\n",
            "  prev=\"$arg\"\n",
            "done\n",
            "file=\"{root}/signature-$tag\"\n",
            "if [ -f \"$file\" ]; then\n",
            "  value=''; code=0\n",
            "  while IFS= read -r line; do\n",
            "    case \"$line\" in\n",
            "      code=*) code=\"${{line#code=}}\" ;;\n",
            "      *) value=\"$line\" ;;\n",
            "    esac\n",
            "  done < \"$file\"\n",
            "  if [ -n \"$value\" ]; then printf '%s\\n' \"$value\"; fi\n",
            "  exit \"$code\"\n",
            "fi\n",
            "exit 2"
        ),
        root = root
    )
}

/// Builds a fake that logs its arguments and exits successfully.
fn fake_logger(root: &str, name: &str) -> String {
    format!("printf '{name} %s\\n' \"$*\" >> '{root}/tools.log'\nexit 0")
}

/// Builds the `sudo` fake, which logs and runs the wrapped command.
fn fake_sudo(root: &str) -> String {
    format!("printf 'sudo %s\\n' \"$*\" >> '{root}/tools.log'\nexec \"$@\"")
}

/// Builds the `chown` fake, whose exit status comes from a state file.
fn fake_chown(root: &str) -> String {
    format!(
        concat!(
            "printf 'chown %s\n' \"$*\" >> '{root}/tools.log'\n",
            "code=0\n",
            "if [ -f '{root}/chown-exit' ]; then read -r code < '{root}/chown-exit'; fi\n",
            "exit \"${{code:-0}}\""
        ),
        root = root
    )
}

/// Builds the `mkfs.ext4` fake, whose exit status comes from a state file.
fn fake_mkfs(root: &str) -> String {
    format!(
        concat!(
            "printf 'mkfs.ext4 %s\\n' \"$*\" >> '{root}/tools.log'\n",
            "code=0\n",
            "if [ -f '{root}/mkfs-exit' ]; then read -r code < '{root}/mkfs-exit'; fi\n",
            "exit \"${{code:-0}}\""
        ),
        root = root
    )
}

/// Builds the `mount` fake, which records the source it was given.
fn fake_mount(root: &str) -> String {
    format!(
        concat!(
            "printf 'mount %s\\n' \"$*\" >> '{root}/tools.log'\n",
            "if [ -f '{root}/mount-source-override' ]; then\n",
            "  while IFS= read -r line; do printf '%s\\n' \"$line\"; done ",
            "< '{root}/mount-source-override' > '{root}/mount-source'\n",
            "else\n",
            "  printf '%s\\n' \"$3\" > '{root}/mount-source'\n",
            "fi\n",
            "exit 0"
        ),
        root = root
    )
}

/// Builds the `umount` fake, whose exit status comes from a state file.
fn fake_umount(root: &str) -> String {
    format!(
        concat!(
            "printf 'umount %s\\n' \"$*\" >> '{root}/tools.log'\n",
            "code=0\n",
            "if [ -f '{root}/umount-exit' ]; then read -r code < '{root}/umount-exit'; fi\n",
            "if [ \"${{code:-0}}\" -ne 0 ]; then exit \"$code\"; fi\n",
            ": > '{root}/mount-source'\n",
            "exit 0"
        ),
        root = root
    )
}

/// Builds the full set of fake tools.
pub(super) fn fakes(root: &str) -> Vec<(&'static str, String)> {
    vec![
        ("lsblk", fake_lsblk(root)),
        ("findmnt", fake_findmnt(root)),
        ("blkid", fake_blkid(root)),
        ("udevadm", fake_logger(root, "udevadm")),
        ("sleep", fake_logger(root, "sleep")),
        ("sync", fake_logger(root, "sync")),
        ("mkdir", fake_logger(root, "mkdir")),
        ("chown", fake_chown(root)),
        ("sudo", fake_sudo(root)),
        ("mkfs.ext4", fake_mkfs(root)),
        ("mount", fake_mount(root)),
        ("umount", fake_umount(root)),
    ]
}
