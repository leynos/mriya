//! Executes the generated cache-device scripts against fake guest utilities.
//!
//! The scripts decide what to format and mount, so text assertions are not
//! enough: this suite runs them under a restricted `PATH` where `lsblk`,
//! `findmnt`, `blkid`, `udevadm`, `sudo`, `mount`, `umount`, `chown`, `mkdir`,
//! `mkfs.ext4`, `sync` and `sleep` are fakes that log every call. The log is
//! the evidence that a refusal never ran a destructive command.

#[path = "cache_device_support/guest.rs"]
mod guest;
#[path = "cache_device_support/sandbox.rs"]
mod sandbox;
#[path = "cache_device_support/shell.rs"]
mod shell;

use anyhow::{Result, ensure};
use guest::{
    Guest,
    MOUNT_PATH,
    RecordType,
    StateFile,
    VOLUME_ID,
    format_case,
    mount_case,
    serial_line,
    serial_record,
    status_of,
};
use mriya::cache_device::unmount_script;
use rstest::rstest;

/// One format scenario, the state it sets, and the status it must report.
struct FormatCase {
    /// State files written before the script runs.
    state: &'static [(&'static str, &'static str)],
    /// Status the script must report.
    status: &'static str,
    /// Whether `mkfs.ext4` must appear in the call log.
    formats: bool,
}

impl FormatCase {
    /// Reports whether this case formats successfully.
    fn succeeds(&self) -> bool { self.status == "formatted" }
}

/// Format scenarios covering every refusal and the formatting path.
const FORMAT_CASES: &[FormatCase] = &[
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
        state: &[("signature-TYPE", "ext4\n")],
        status: "signature-present",
        formats: false,
    },
    FormatCase {
        state: &[("signature-PTTYPE", "gpt\n")],
        status: "signature-present",
        formats: false,
    },
    FormatCase {
        state: &[("signature-TYPE", "code=8\n")],
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
    // `blkid` returns 2 for an unreadable device as well as for an empty one,
    // so a failed read must stop the format before `mkfs.ext4` runs.
    FormatCase {
        state: &[("dd-exit", "1\n")],
        status: "device-unreadable",
        formats: false,
    },
    // A partition record carrying the serial is not the cache disk, so the
    // resolver must ignore it.
    FormatCase {
        state: &[("scsi", "partition-record")],
        status: "not-found",
        formats: false,
    },
    // A UUID surviving from a partially wiped filesystem is still a signature.
    FormatCase {
        state: &[("signature-UUID", "a23cd711-3a36-4fca-bfa9-fab507e19e6f\n")],
        status: "signature-present",
        formats: false,
    },
];

/// Applies one format case's state to a fresh guest.
fn apply_format_state(guest: &Guest, case: &FormatCase) -> Result<()> {
    for &(name, raw) in case.state {
        let value = raw.trim_end();
        match name {
            "scsi" if value == "duplicate" => {
                let duplicate = format!(
                    "{}{}",
                    serial_line("sdb", VOLUME_ID),
                    serial_line("sdc", VOLUME_ID)
                );
                guest.set(StateFile::Scsi(&duplicate))?;
            }
            "scsi" if value == "root-disk" => {
                guest.set(StateFile::Scsi(&serial_line("sda", VOLUME_ID)))?;
            }
            "scsi" if value == "root-partition" => {
                guest.set(StateFile::Scsi(&serial_line("sda1", VOLUME_ID)))?;
            }
            "scsi" if value == "partition-record" => {
                guest.set(StateFile::Scsi(&serial_record(
                    "sdb",
                    VOLUME_ID,
                    RecordType::Part,
                )))?;
            }
            "scsi" => guest.set(StateFile::Scsi(value))?,
            "root-source" => guest.set(StateFile::RootSource(value))?,
            "pkname-vg-root" => guest.set(StateFile::PknameVgRoot(value))?,
            "pkname-dm-0" => guest.set(StateFile::PknameDm0(value))?,
            "signature-TYPE" => guest.set(StateFile::SignatureType(value))?,
            "signature-PTTYPE" => guest.set(StateFile::SignaturePartitionTable(value))?,
            "signature-UUID" => guest.set(StateFile::SignatureUuid(value))?,
            "findmnt-exit" => guest.set(StateFile::FindmntExit(value))?,
            "mountpoint-exit" => guest.set(StateFile::MountpointExit(value))?,
            "dd-exit" => guest.set(StateFile::DdExit(value))?,
            "mkfs-exit" => guest.set(StateFile::MkfsExit(value))?,
            "umount-exit" => guest.set(StateFile::UmountExit(value))?,
            "chown-exit" => guest.set(StateFile::ChownExit(value))?,
            "mount-source" => guest.set(StateFile::MountSource(value))?,
            "mount-source-override" => {
                guest.set(StateFile::MountSourceOverride(value))?;
            }
            _ => return Err(anyhow::anyhow!("unknown state file: {name}")),
        }
    }
    Ok(())
}

#[rstest]
fn format_refuses_unsafe_devices(
    #[values(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13)] index: usize,
) -> Result<()> {
    let case = FORMAT_CASES
        .get(index)
        .ok_or_else(|| anyhow::anyhow!("format case {index} is out of range"))?;
    let guest = Guest::new()?;
    apply_format_state(&guest, case)?;
    ensure!(
        format_case(&guest)? == case.status,
        "case {index} expected {}",
        case.status
    );
    let log = guest.log()?;
    ensure!(
        log.contains("mkfs.ext4") == case.formats,
        "case {index} formatting mismatch: {log}"
    );
    if case.formats {
        ensure!(
            log.contains("mkfs.ext4 -F -L mriya-cache /dev/sdb"),
            "{log}"
        );
    }
    if case.succeeds() {
        ensure!(log.contains("sync"), "{log}");
    }
    Ok(())
}

/// One mount scenario, the state it sets, and the status it must report.
struct MountCase {
    /// State files written before the script runs.
    state: &'static [(&'static str, &'static str)],
    /// Status the script must report.
    status: &'static str,
    /// Whether the mount command must appear in the call log.
    mounts: bool,
}

/// Mount scenarios covering every refusal and the verified path.
const MOUNT_CASES: &[MountCase] = &[
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

#[rstest]
fn mount_refuses_unsafe_sources(#[values(0, 1, 2, 3, 4, 5, 6, 7, 8)] index: usize) -> Result<()> {
    let case = MOUNT_CASES
        .get(index)
        .ok_or_else(|| anyhow::anyhow!("mount case {index} is out of range"))?;
    let guest = Guest::new()?;
    apply_format_state(
        &guest,
        &FormatCase {
            state: case.state,
            status: "",
            formats: false,
        },
    )?;
    let observed = mount_case(&guest)?;
    ensure!(
        observed == case.status,
        "case {index} expected {} but observed {observed}",
        case.status
    );
    let log = guest.log()?;
    ensure!(
        log.contains("mount -o defaults") == case.mounts,
        "case {index} mount mismatch: {log}"
    );
    if case.mounts {
        ensure!(log.contains("mount -o defaults /dev/sdb /mriya"), "{log}");
    }
    if case.status == "verified" {
        ensure!(log.contains("chown ubuntu:ubuntu /mriya"), "{log}");
    }
    Ok(())
}

#[test]
fn mount_refuses_when_ownership_fails() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(StateFile::SignatureType("ext4"))?;
    guest.set(StateFile::ChownExit("1"))?;
    ensure!(mount_case(&guest)? == "ownership-failed");
    ensure!(guest.log()?.contains("chown"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn mount_refuses_the_root_disk_without_mounting() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(StateFile::Scsi(&serial_line("sda", VOLUME_ID)))?;
    guest.set(StateFile::SignatureType("ext4"))?;
    ensure!(mount_case(&guest)? == "root-disk");
    let log = guest.log()?;
    ensure!(!log.contains("mount -o defaults"), "{log}");
    ensure!(!log.contains("chown"), "{log}");
    Ok(())
}

#[test]
fn unmount_flushes_and_unmounts() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(StateFile::MountSource("/dev/sdb"))?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "unmounted");
    let log = guest.log()?;
    ensure!(log.contains("sync"), "{log}");
    ensure!(log.contains("umount /mriya"), "{log}");
    Ok(())
}

#[test]
fn unmount_refuses_a_foreign_source() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(StateFile::MountSource("/dev/sdc"))?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "wrong-source");
    ensure!(!guest.log()?.contains("umount"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn unmount_refuses_a_missing_serial() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(StateFile::Scsi(""))?;
    guest.set(StateFile::MountSource("/dev/sdb"))?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "not-found");
    ensure!(!guest.log()?.contains("umount"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn unmount_reports_an_unresolvable_mount_as_a_failed_query() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(StateFile::FindmntExit("4"))?;
    // A failing query must not be read as "nothing mounted": teardown would
    // then detach the volume under a live filesystem.
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "query-failed");
    ensure!(!guest.log()?.contains("umount"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn unmount_reports_a_path_that_stays_mounted_without_a_source() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(StateFile::MountpointExit("0"))?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "query-failed");
    ensure!(!guest.log()?.contains("umount"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn unmount_reports_a_failure() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(StateFile::MountSource("/dev/sdb"))?;
    guest.set(StateFile::UmountExit("1"))?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "unmount-failed");
    Ok(())
}
