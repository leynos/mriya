//! Fake guest utilities for the cache-device shell tests.
//!
//! Each fake logs its arguments to a shared file and reads its state from small
//! files under the sandbox root, so a case can model a filesystem, a
//! partition table, an ambiguous probe, a device-mapper root, a duplicate
//! serial or a failing command without touching the host. The fakes share one
//! sandbox root, so they are methods on [`Fakes`] rather than free functions
//! that repeat the root argument.

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

/// Fake guest utilities rooted at one sandbox directory.
struct Fakes<'a> {
    /// Sandbox root holding the state files and the call log.
    root: &'a str,
}

/// Builds the full set of fake tools for `root`.
pub(super) fn fakes(root: &str) -> Vec<(&'static str, String)> { Fakes { root }.all() }

impl Fakes<'_> {
    /// Returns every fake in the order they are written.
    fn all(&self) -> Vec<(&'static str, String)> {
        vec![
            ("lsblk", self.lsblk()),
            ("findmnt", self.findmnt()),
            ("blkid", self.blkid()),
            ("mountpoint", self.mountpoint()),
            ("blockdev", self.blockdev()),
            ("wipefs", self.wipefs()),
            ("dd", self.dd()),
            ("udevadm", self.logger("udevadm")),
            ("sleep", self.logger("sleep")),
            ("sync", self.logger("sync")),
            ("mkdir", self.logger("mkdir")),
            ("chown", self.exit_status("chown", "chown-exit")),
            ("sudo", self.sudo()),
            ("mkfs.ext4", self.exit_status("mkfs.ext4", "mkfs-exit")),
            ("mount", self.mount()),
            ("umount", self.umount()),
        ]
    }

    /// Builds the resolver and root-guard `lsblk` fake.
    fn lsblk(&self) -> String {
        let root = self.root;
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
                "    if [ -f '{root}/pkname-exit' ]; then ",
                "read -r code < '{root}/pkname-exit'; exit \"$code\"; fi\n",
                "    exit 0\n",
                "    ;;\n",
                "  *)\n",
                "    {scsi}\n",
                "    if [ -f '{root}/lsblk-exit' ]; then ",
                "read -r code < '{root}/lsblk-exit'; exit \"$code\"; fi\n",
                "    exit 0\n",
                "    ;;\n",
                "esac"
            ),
            root = root,
            scsi = read_state(root, "scsi"),
        )
    }

    /// Builds the `findmnt` fake for root-source and mount-point queries.
    ///
    /// A mount-point query with no source exits 1, as the real `findmnt` does
    /// when the lookup finds nothing, so the script's "not mounted" branch is
    /// exercised rather than masked by a successful empty result. A
    /// `findmnt-exit` state models a failing query.
    fn findmnt(&self) -> String {
        let root = self.root;
        format!(
            concat!(
                "printf 'findmnt %s\\n' \"$*\" >> '{root}/tools.log'\n",
                "case \"$*\" in\n",
                "  *--mountpoint*)\n",
                "    {mount}\n",
                "    if [ -f '{root}/findmnt-exit' ]; then ",
                "read -r findmnt_code < '{root}/findmnt-exit'; exit \"$findmnt_code\"; fi\n",
                "    if [ -s '{root}/mount-source' ]; then exit 0; fi\n",
                "    exit 1\n",
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
    /// The fake distinguishes the `TYPE`, `UUID` and `PTTYPE` probes so a case
    /// can model a filesystem, a partition table, an ambiguous low-level result
    /// or an operational failure independently.
    fn blkid(&self) -> String {
        let root = self.root;
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

    /// Logs one call and exits with a configured status when one is set.
    fn log_with_override(&self, tool: &str, state: &str) -> String {
        format!(
            concat!(
                "printf '{tool} %s\\n' \"$*\" >> '{root}/tools.log'\n",
                "if [ -f '{root}/{state}' ]; then ",
                "read -r code < '{root}/{state}'; exit \"$code\"; fi\n",
            ),
            root = self.root,
            tool = tool,
            state = state,
        )
    }

    /// Builds the `mountpoint` fake, whose status comes from a state file.
    ///
    /// A fresh guest has no mount-point state, so the fake reports 32 ("not a
    /// mount point") rather than success, matching the real tool for a path
    /// that exists but is not mounted.
    fn mountpoint(&self) -> String {
        format!(
            concat!(
                "{logged}",
                "if [ -f '{root}/mount-source' ] && [ -s '{root}/mount-source' ]; ",
                "then exit 0; fi\n",
                "exit 32"
            ),
            logged = self.log_with_override("mountpoint", "mountpoint-exit"),
            root = self.root,
        )
    }

    /// Builds the `wipefs` fake, which lists the configured signatures.
    ///
    /// An empty listing is a clean, empty disk; a non-empty listing is a
    /// signature; a `wipefs-exit` state models a failed probe.
    fn wipefs(&self) -> String {
        format!(
            concat!(
                "{logged}",
                "if [ -f '{root}/wipefs-output' ]; then ",
                "while IFS= read -r line; do printf '%s\\n' \"$line\"; done ",
                "< '{root}/wipefs-output'; exit 0; fi\n",
                "exit 0"
            ),
            logged = self.log_with_override("wipefs", "wipefs-exit"),
            root = self.root,
        )
    }

    /// Builds the `blockdev` fake, whose size and status come from state.
    fn blockdev(&self) -> String {
        let root = self.root;
        format!(
            concat!(
                "printf 'blockdev %s\\n' \"$*\" >> '{root}/tools.log'\n",
                "code=0\n",
                "if [ -f '{root}/blockdev-exit' ]; then ",
                "read -r code < '{root}/blockdev-exit'; fi\n",
                "if [ \"${{code:-0}}\" -ne 0 ]; then exit \"$code\"; fi\n",
                "size=10485760\n",
                "if [ -f '{root}/blockdev-size' ]; then ",
                "read -r size < '{root}/blockdev-size'; fi\n",
                "printf '%s\\n' \"$size\""
            ),
            root = root
        )
    }

    /// Builds the `dd` fake used by the readability probe.
    fn dd(&self) -> String { self.exit_status("dd", "dd-exit") }

    /// Builds a fake that logs its arguments and exits successfully.
    fn logger(&self, name: &str) -> String {
        format!(
            "printf '{name} %s\\n' \"$*\" >> '{root}/tools.log'\nexit 0",
            root = self.root
        )
    }

    /// Builds a fake whose exit status comes from `state`.
    fn exit_status(&self, name: &str, state: &str) -> String {
        let root = self.root;
        format!(
            concat!(
                "printf '{name} %s\\n' \"$*\" >> '{root}/tools.log'\n",
                "code=0\n",
                "if [ -f '{root}/{state}' ]; then read -r code < '{root}/{state}'; fi\n",
                "exit \"${{code:-0}}\""
            ),
            root = root,
            name = name,
            state = state,
        )
    }

    /// Builds the `sudo` fake, which logs and runs the wrapped command.
    fn sudo(&self) -> String {
        format!(
            "printf 'sudo %s\\n' \"$*\" >> '{root}/tools.log'\nexec \"$@\"",
            root = self.root
        )
    }

    /// Builds the `mount` fake, which records the source it was given.
    fn mount(&self) -> String {
        let root = self.root;
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
    fn umount(&self) -> String {
        let root = self.root;
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
}
