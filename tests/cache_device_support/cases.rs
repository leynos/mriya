//! Scenario tables for the cache-device shell tests.
//!
//! Keeping the tables here separates the safety cases from the harness that
//! runs them, so the test module stays within the file-size budget.

/// One format scenario, the state it sets, and the status it must report.
pub(super) struct FormatCase {
    /// State files written before the script runs.
    pub(super) state: &'static [(&'static str, &'static str)],
    /// Status the script must report.
    pub(super) status: &'static str,
    /// Whether `mkfs.ext4` must appear in the call log.
    pub(super) formats: bool,
}

impl FormatCase {
    /// Reports whether this case formats successfully.
    pub(super) fn succeeds(&self) -> bool { self.status == "formatted" }
}

/// Format scenarios covering every refusal and the formatting path.
pub(super) const FORMAT_CASES: &[FormatCase] = &[
    FormatCase {
        state: &[],
        status: "formatted",
        formats: true,
    },
    FormatCase {
        state: &[("scsi", "")],
        status: "not-found",
        formats: false,
    },
    FormatCase {
        state: &[("scsi", "duplicate")],
        status: "ambiguous",
        formats: false,
    },
    FormatCase {
        state: &[("scsi", "root-disk")],
        status: "root-disk",
        formats: false,
    },
    FormatCase {
        state: &[("scsi", "root-partition")],
        status: "root-disk",
        formats: false,
    },
    FormatCase {
        state: &[("wipefs-output", "disk ext4\n")],
        status: "signature-present",
        formats: false,
    },
    FormatCase {
        state: &[("wipefs-output", "disk gpt\n")],
        status: "signature-present",
        formats: false,
    },
    FormatCase {
        state: &[("wipefs-exit", "1\n")],
        status: "probe-failed",
        formats: false,
    },
    FormatCase {
        state: &[
            ("root-source", "/dev/mapper/vg-root\n"),
            ("pkname-vg-root", "dm-0\n"),
            ("pkname-dm-0", "sda\n"),
            ("scsi", "root-disk"),
        ],
        status: "root-disk",
        formats: false,
    },
    FormatCase {
        state: &[("root-source", "/dev/mapper/vg-root\n")],
        status: "root-unknown",
        formats: false,
    },
    FormatCase {
        state: &[("mkfs-exit", "1\n")],
        status: "format-failed",
        formats: true,
    },
    // A failed signature probe must stop the format even when a later read
    // would succeed: only a successful empty probe authorizes `mkfs.ext4`.
    FormatCase {
        state: &[("wipefs-exit", "2\n")],
        status: "probe-failed",
        formats: false,
    },
    // An empty `wipefs` listing is not proof on its own: a blkid probe that
    // cannot return a clean negative must stop the format.
    FormatCase {
        state: &[("signature-TYPE", "code=8\n")],
        status: "probe-failed",
        formats: false,
    },
    // Nor is it proof when the signature-bearing regions cannot be read.
    FormatCase {
        state: &[("dd-exit", "1\n")],
        status: "probe-failed",
        formats: false,
    },
    // A failed device-size query cannot prove where the tail region is.
    FormatCase {
        state: &[("blockdev-exit", "1\n")],
        status: "probe-failed",
        formats: false,
    },
    // A partition record carrying the serial is not the cache disk, so the
    // resolver must ignore it.
    FormatCase {
        state: &[("scsi", "partition-record")],
        status: "not-found",
        formats: false,
    },
    // A failed disk listing cannot prove the device is absent.
    FormatCase {
        state: &[("lsblk-exit", "1\n")],
        status: "probe-failed",
        formats: false,
    },
    // A failed ancestry read cannot prove the candidate is not the root disk.
    FormatCase {
        state: &[("pkname-exit", "1\n")],
        status: "root-unknown",
        formats: false,
    },
    // A multi-parent root source cannot be resolved to one disk.
    FormatCase {
        state: &[("pkname-sda1", "sda\nsdb\n")],
        status: "root-unknown",
        formats: false,
    },
    // Any listed signature blocks formatting, including a surviving UUID.
    FormatCase {
        state: &[(
            "wipefs-output",
            "disk ext4 a23cd711-3a36-4fca-bfa9-fab507e19e6f\n",
        )],
        status: "signature-present",
        formats: false,
    },
];

/// One mount scenario, the state it sets, and the status it must report.
pub(super) struct MountCase {
    /// State files written before the script runs.
    pub(super) state: &'static [(&'static str, &'static str)],
    /// Status the script must report.
    pub(super) status: &'static str,
    /// Whether the mount command must appear in the call log.
    pub(super) mounts: bool,
}

/// Mount scenarios covering every refusal and the verified path.
pub(super) const MOUNT_CASES: &[MountCase] = &[
    MountCase {
        state: &[("signature-TYPE", "ext4\n")],
        status: "verified",
        mounts: true,
    },
    MountCase {
        state: &[("signature-TYPE", "xfs\n")],
        status: "not-ext4",
        mounts: false,
    },
    MountCase {
        state: &[("signature-TYPE", "ext4\n"), ("signature-PTTYPE", "gpt\n")],
        status: "not-ext4",
        mounts: false,
    },
    MountCase {
        state: &[("signature-TYPE", "code=8\n")],
        status: "probe-failed",
        mounts: false,
    },
    MountCase {
        state: &[("signature-TYPE", "ext4\n"), ("mount-source", "/dev/sdc\n")],
        status: "mount-occupied",
        mounts: false,
    },
    MountCase {
        state: &[
            ("signature-TYPE", "ext4\n"),
            ("mount-source-override", "/dev/sdc\n"),
        ],
        status: "wrong-source",
        // This case mounts the device, then refuses because the mount source
        // is not the resolved device.
        mounts: true,
    },
    MountCase {
        state: &[("scsi", "root-disk"), ("signature-TYPE", "ext4\n")],
        status: "root-disk",
        mounts: false,
    },
    MountCase {
        state: &[("signature-TYPE", "ext4\n"), ("findmnt-exit", "3\n")],
        status: "query-failed",
        mounts: false,
    },
    MountCase {
        // findmnt reports "nothing mounted" while mountpoint says the path is
        // still a mount point: mounting over it would hide the filesystem.
        state: &[
            ("signature-TYPE", "ext4\n"),
            ("findmnt-exit", "1\n"),
            ("mountpoint-exit", "0\n"),
        ],
        status: "query-failed",
        mounts: false,
    },
];
