//! Executes the generated cache-device scripts against fake guest utilities.
//!
//! The scripts decide what to format and mount, so text assertions are not
//! enough: this suite runs them under a restricted `PATH` where `lsblk`,
//! `findmnt`, `blkid`, `udevadm`, `sudo`, `mount`, `umount`, `chown`, `mkdir`,
//! `mkfs.ext4`, `sync` and `sleep` are fakes that log every call. The log is
//! the evidence that a refusal never ran a destructive command.

#[path = "cache_device_support/cases.rs"]
mod cases;
#[path = "cache_device_support/guest.rs"]
mod guest;
#[path = "cache_device_support/sandbox.rs"]
mod sandbox;
#[path = "cache_device_support/shell.rs"]
mod shell;

use anyhow::{Result, ensure};
use cases::{FORMAT_CASES, FormatCase, MOUNT_CASES};
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

/// Applies one format case's state to a fresh guest.
fn apply_format_state(guest: &Guest, case: &FormatCase) -> Result<()> {
    for &(name, raw) in case.state {
        let value = raw.trim_end();
        match name {
            "scsi" => match value {
                "duplicate" => {
                    let duplicate = format!(
                        "{}{}",
                        serial_line("sdb", VOLUME_ID),
                        serial_line("sdc", VOLUME_ID)
                    );
                    guest.set(StateFile::Scsi(&duplicate))?;
                }
                "root-disk" => guest.set(StateFile::Scsi(&serial_line("sda", VOLUME_ID)))?,
                "root-partition" => {
                    guest.set(StateFile::Scsi(&serial_line("sda1", VOLUME_ID)))?;
                }
                "partition-record" => {
                    guest.set(StateFile::Scsi(&serial_record(
                        "sdb",
                        VOLUME_ID,
                        RecordType::Part,
                    )))?;
                }
                other => guest.set(StateFile::Scsi(other))?,
            },
            _ => match StateFile::from_case(name, value) {
                Some(state) => guest.set(state)?,
                None => return Err(anyhow::anyhow!("unknown state file: {name}")),
            },
        }
    }
    Ok(())
}

#[rstest]
fn format_refuses_unsafe_devices(
    #[values(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19)] index: usize,
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
        ensure!(log.contains("chown ubuntu /mriya"), "{log}");
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
