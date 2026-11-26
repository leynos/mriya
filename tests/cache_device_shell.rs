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

use anyhow::{Result, ensure};
use guest::{Guest, MOUNT_PATH, VOLUME_ID, format_case, mount_case, serial_line, status_of};
use mriya::cache_device::unmount_script;
use rstest::rstest;

#[test]
fn format_resolves_the_serial_and_formats_the_disk() -> Result<()> {
    let guest = Guest::new()?;
    ensure!(format_case(&guest)? == "formatted", "expected formatted");
    let log = guest.log()?;
    ensure!(
        log.contains("mkfs.ext4 -F -L mriya-cache /dev/sdb"),
        "{log}"
    );
    ensure!(log.contains("sync"), "{log}");
    Ok(())
}

#[test]
fn format_refuses_a_missing_serial_without_formatting() -> Result<()> {
    let guest = Guest::new()?;
    guest.clear("scsi")?;
    ensure!(format_case(&guest)? == "not-found");
    ensure!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_refuses_a_duplicate_serial_without_formatting() -> Result<()> {
    let guest = Guest::new()?;
    guest.set(
        "scsi",
        &format!(
            "{}{}",
            serial_line("sdb", VOLUME_ID),
            serial_line("sdc", VOLUME_ID)
        ),
    )?;
    ensure!(format_case(&guest)? == "ambiguous");
    ensure!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[rstest]
#[case::root_disk("sda", "root-disk")]
#[case::root_partition("sda1", "root-disk")]
fn format_refuses_the_root_device(#[case] disk: &str, #[case] expected: &str) -> Result<()> {
    let guest = Guest::new()?;
    guest.set("scsi", &serial_line(disk, VOLUME_ID))?;
    ensure!(format_case(&guest)? == expected);
    ensure!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_refuses_an_existing_signature() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "ext4\n")?;
    ensure!(format_case(&guest)? == "signature-present");
    ensure!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_refuses_a_partition_table() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-PTTYPE", "gpt\n")?;
    ensure!(format_case(&guest)? == "signature-present");
    ensure!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_refuses_an_ambiguous_probe() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "code=8\n")?;
    ensure!(format_case(&guest)? == "probe-failed");
    ensure!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_refuses_a_device_mapper_root_disk() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("root-source", "/dev/mapper/vg-root\n")?;
    guest.set("pkname-vg-root", "dm-0\n")?;
    guest.set("pkname-dm-0", "sda\n")?;
    guest.set("scsi", &serial_line("sda", VOLUME_ID))?;
    ensure!(format_case(&guest)? == "root-disk");
    ensure!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_refuses_an_unresolvable_root() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("root-source", "/dev/mapper/vg-root\n")?;
    ensure!(format_case(&guest)? == "root-unknown");
    ensure!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_reports_a_mkfs_failure() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("mkfs-exit", "1\n")?;
    ensure!(format_case(&guest)? == "format-failed");
    ensure!(guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn mount_verifies_the_source_and_sets_ownership() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "ext4\n")?;
    ensure!(mount_case(&guest)? == "verified");
    let log = guest.log()?;
    ensure!(log.contains("mount -o defaults /dev/sdb /mriya"), "{log}");
    ensure!(log.contains("chown ubuntu:ubuntu /mriya"), "{log}");
    Ok(())
}

#[test]
fn mount_refuses_a_non_ext4_source() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "xfs\n")?;
    ensure!(mount_case(&guest)? == "not-ext4");
    ensure!(
        !guest.log()?.contains("mount -o defaults"),
        "{}",
        guest.log()?
    );
    Ok(())
}

#[test]
fn mount_refuses_an_occupied_path() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "ext4\n")?;
    guest.set("mount-source", "/dev/sdc\n")?;
    ensure!(mount_case(&guest)? == "mount-occupied");
    ensure!(
        !guest.log()?.contains("mount -o defaults"),
        "{}",
        guest.log()?
    );
    Ok(())
}

#[test]
fn mount_refuses_a_wrong_source_without_chowning() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "ext4\n")?;
    guest.set("mount-source-override", "/dev/sdc\n")?;
    ensure!(mount_case(&guest)? == "wrong-source");
    ensure!(!guest.log()?.contains("chown"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn mount_refuses_a_partitioned_device() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "ext4\n")?;
    guest.set("signature-PTTYPE", "gpt\n")?;
    ensure!(mount_case(&guest)? == "not-ext4");
    ensure!(
        !guest.log()?.contains("mount -o defaults"),
        "{}",
        guest.log()?
    );
    Ok(())
}

#[test]
fn mount_refuses_an_ambiguous_probe() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "code=8\n")?;
    ensure!(mount_case(&guest)? == "probe-failed");
    ensure!(
        !guest.log()?.contains("mount -o defaults"),
        "{}",
        guest.log()?
    );
    Ok(())
}

#[test]
fn mount_refuses_when_ownership_fails() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature-TYPE", "ext4\n")?;
    guest.set("chown-exit", "1\n")?;
    ensure!(mount_case(&guest)? == "ownership-failed");
    ensure!(guest.log()?.contains("chown"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn mount_refuses_the_root_disk_without_mounting() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("scsi", &serial_line("sda", VOLUME_ID))?;
    guest.set("signature-TYPE", "ext4\n")?;
    ensure!(mount_case(&guest)? == "root-disk");
    let log = guest.log()?;
    ensure!(!log.contains("mount -o defaults"), "{log}");
    ensure!(!log.contains("chown"), "{log}");
    Ok(())
}

#[test]
fn unmount_flushes_and_unmounts() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("mount-source", "/dev/sdb\n")?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "unmounted");
    let log = guest.log()?;
    ensure!(log.contains("sync"), "{log}");
    ensure!(log.contains("umount /mriya"), "{log}");
    Ok(())
}

#[test]
fn unmount_refuses_a_foreign_source() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("mount-source", "/dev/sdc\n")?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "wrong-source");
    ensure!(!guest.log()?.contains("umount"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn unmount_refuses_a_missing_serial() -> Result<()> {
    let guest = Guest::new()?;
    guest.clear("scsi")?;
    guest.set("mount-source", "/dev/sdb\n")?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "not-found");
    ensure!(!guest.log()?.contains("umount"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn unmount_reports_a_failure() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("mount-source", "/dev/sdb\n")?;
    guest.set("umount-exit", "1\n")?;
    ensure!(status_of(&unmount_script(VOLUME_ID, MOUNT_PATH), &guest)? == "unmount-failed");
    Ok(())
}
