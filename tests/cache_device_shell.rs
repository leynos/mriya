//! Executes the generated cache-device scripts against fake guest utilities.
//!
//! The scripts decide what to format and mount, so text assertions are not
//! enough: this suite runs them under a restricted `PATH` where `lsblk`,
//! `findmnt`, `blkid`, `udevadm`, `sudo`, `mount`, `umount`, `chown`, `mkdir`,
//! `mkfs.ext4`, `sync` and `sleep` are fakes that log every call. The log is
//! the evidence that a refusal never ran a destructive command.

#[path = "cache_device_support/sandbox.rs"]
mod sandbox;

use mriya::cache_device::{format_script, mount_script, parse_report, unmount_script};
use rstest::rstest;
use sandbox::Sandbox;

/// Volume UUID used by every case.
const VOLUME_ID: &str = "11111111-1111-1111-1111-111111111111";
/// Mount path used by the mount and unmount cases.
const MOUNT_PATH: &str = "/mriya";

/// A restricted `PATH` of fake guest utilities backed by state files.
struct Guest {
    sandbox: Sandbox,
}

impl Guest {
    fn new() -> Result<Self, String> {
        let sandbox = Sandbox::new()?;
        sandbox.link_tools(&["bash", "awk"])?;
        sandbox.write_file("root-source", b"/dev/sda1\n")?;
        sandbox.write_file("pkname-sda1", b"sda\n")?;
        sandbox.write_file("scsi", serial_line("sdb", VOLUME_ID).as_bytes())?;
        sandbox.write_file("mkfs-exit", b"0\n")?;
        sandbox.write_file("umount-exit", b"0\n")?;

        for (name, body) in fakes(sandbox.root()) {
            sandbox.write_tool(name, body)?;
        }
        Ok(Self { sandbox })
    }

    fn set(&self, name: &str, contents: &str) -> Result<(), String> {
        self.sandbox.write_file(name, contents.as_bytes())
    }

    fn clear(&self, name: &str) -> Result<(), String> { self.sandbox.write_file(name, b"") }

    fn run(&self, script: &str) -> Result<(String, Option<i32>), String> {
        let mut command = self.sandbox.command()?;
        command.arg("-c").arg(script);
        let output = command
            .output()
            .map_err(|error| format!("running the script: {error}"))?;
        Ok((
            String::from_utf8_lossy(&output.stdout).into_owned(),
            output.status.code(),
        ))
    }

    fn log(&self) -> Result<String, String> {
        Ok(self.sandbox.read("tools.log")?.unwrap_or_default())
    }
}

/// Renders one `lsblk -P` record for a disk.
fn serial_line(name: &str, volume_id: &str) -> String {
    format!(
        "NAME=\"{name}\" VENDOR=\"SCW\" MODEL=\"sbs\" SERIAL=\"volume-{volume_id}\" TYPE=\"disk\"\n"
    )
}

/// Builds the fake tool scripts, embedding the sandbox root as `root`.
fn fakes(root: &str) -> Vec<(&'static str, String)> {
    let read_file = |name: &str| {
        format!(
            concat!(
                "if [ -f '{root}/{name}' ]; then ",
                "while IFS= read -r line; do printf '%s\\n' \"$line\"; done < '{root}/{name}'; fi"
            ),
            root = root,
            name = name,
        )
    };
    vec![
        (
            "lsblk",
            format!(
                concat!(
                    "printf 'lsblk %s\\n' \"$*\" >> '{root}/tools.log'\n",
                    "case \"$*\" in\n",
                    "  *PKNAME*)\n",
                    "    for arg in \"$@\"; do target=\"$arg\"; done\n",
                    "    name=\"${{target##*/}}\"\n",
                    "    file=\"{root}/pkname-$name\"\n",
                    "    if [ -f \"$file\" ]; then ",
                    "while IFS= read -r line; do printf '%s\\n' \"$line\"; done < \"$file\"; fi\n",
                    "    ;;\n",
                    "  *)\n",
                    "    {scsi}\n",
                    "    ;;\n",
                    "esac"
                ),
                root = root,
                scsi = read_file("scsi"),
            ),
        ),
        (
            "findmnt",
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
                mount = read_file("mount-source"),
                root_source = read_file("root-source"),
            ),
        ),
        (
            "blkid",
            format!(
                concat!(
                    "printf 'blkid %s\\n' \"$*\" >> '{root}/tools.log'\n",
                    "if [ -f '{root}/signature' ]; then {signature}; else exit 2; fi"
                ),
                root = root,
                signature = read_file("signature"),
            ),
        ),
        (
            "udevadm",
            format!("printf 'udevadm %s\\n' \"$*\" >> '{root}/tools.log'\nexit 0"),
        ),
        (
            "sleep",
            format!("printf 'sleep %s\\n' \"$*\" >> '{root}/tools.log'\nexit 0"),
        ),
        (
            "sync",
            format!("printf 'sync %s\\n' \"$*\" >> '{root}/tools.log'\nexit 0"),
        ),
        (
            "sudo",
            format!("printf 'sudo %s\\n' \"$*\" >> '{root}/tools.log'\nexec \"$@\""),
        ),
        (
            "mkdir",
            format!("printf 'mkdir %s\\n' \"$*\" >> '{root}/tools.log'\nexit 0"),
        ),
        (
            "chown",
            format!("printf 'chown %s\\n' \"$*\" >> '{root}/tools.log'\nexit 0"),
        ),
        (
            "mkfs.ext4",
            format!(
                concat!(
                    "printf 'mkfs.ext4 %s\\n' \"$*\" >> '{root}/tools.log'\n",
                    "code=0\n",
                    "if [ -f '{root}/mkfs-exit' ]; then read -r code < '{root}/mkfs-exit'; fi\n",
                    "exit \"${{code:-0}}\""
                ),
                root = root
            ),
        ),
        (
            "mount",
            format!(
                concat!(
                    "printf 'mount %s\\n' \"$*\" >> '{root}/tools.log'\n",
                    "if [ -f '{root}/mount-source-override' ]; then\n",
                    "  while IFS= read -r line; do printf '%s\\n' \"$line\"; done < '{root}/mount-source-override' > '{root}/mount-source'\n",
                    "else\n",
                    "  printf '%s\\n' \"$3\" > '{root}/mount-source'\n",
                    "fi\n",
                    "exit 0"
                ),
                root = root
            ),
        ),
        (
            "umount",
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
            ),
        ),
    ]
}

/// Runs a script and requires the parsed status.
fn status_of(script: &str, guest: &Guest) -> Result<String, String> {
    let (stdout, code) = guest.run(script)?;
    if code != Some(0) {
        return Err(format!("script exited {code:?}: {stdout}"));
    }
    let report = parse_report(&stdout).map_err(|error| format!("{error}: {stdout}"))?;
    Ok(report.status().as_str().to_owned())
}

fn format_case(guest: &Guest) -> Result<String, String> {
    status_of(&format_script(VOLUME_ID), guest)
}

fn mount_case(guest: &Guest) -> Result<String, String> {
    status_of(&mount_script(VOLUME_ID, MOUNT_PATH, "ubuntu"), guest)
}

#[test]
fn format_resolves_the_serial_and_formats_the_disk() -> Result<(), String> {
    let guest = Guest::new()?;
    assert_eq!(format_case(&guest)?, "formatted");
    let log = guest.log()?;
    assert!(
        log.contains("mkfs.ext4 -F -L mriya-cache /dev/sdb"),
        "{log}"
    );
    assert!(log.contains("sync"), "{log}");
    Ok(())
}

#[test]
fn format_refuses_a_missing_serial_without_formatting() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.clear("scsi")?;
    assert_eq!(format_case(&guest)?, "not-found");
    assert!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_refuses_a_duplicate_serial_without_formatting() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set(
        "scsi",
        &format!(
            "{}{}",
            serial_line("sdb", VOLUME_ID),
            serial_line("sdc", VOLUME_ID)
        ),
    )?;
    assert_eq!(format_case(&guest)?, "ambiguous");
    assert!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[rstest]
#[case::root_disk("sda", "root-disk")]
#[case::root_partition("sda1", "root-disk")]
fn format_refuses_the_root_device(
    #[case] disk: &str,
    #[case] expected: &str,
) -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("scsi", &serial_line(disk, VOLUME_ID))?;
    assert_eq!(format_case(&guest)?, expected);
    assert!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_refuses_an_existing_signature() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("signature", "ext4\n")?;
    assert_eq!(format_case(&guest)?, "signature-present");
    assert!(!guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn format_reports_a_mkfs_failure() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("mkfs-exit", "1\n")?;
    assert_eq!(format_case(&guest)?, "format-failed");
    assert!(guest.log()?.contains("mkfs.ext4"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn mount_verifies_the_source_and_sets_ownership() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("signature", "ext4\n")?;
    assert_eq!(mount_case(&guest)?, "verified");
    let log = guest.log()?;
    assert!(log.contains("mount -o defaults /dev/sdb /mriya"), "{log}");
    assert!(log.contains("chown ubuntu:ubuntu /mriya"), "{log}");
    Ok(())
}

#[test]
fn mount_refuses_a_non_ext4_source() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("signature", "xfs\n")?;
    assert_eq!(mount_case(&guest)?, "not-ext4");
    assert!(
        !guest.log()?.contains("mount -o defaults"),
        "{}",
        guest.log()?
    );
    Ok(())
}

#[test]
fn mount_refuses_an_occupied_path() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("signature", "ext4\n")?;
    guest.set("mount-source", "/dev/sdc\n")?;
    assert_eq!(mount_case(&guest)?, "mount-occupied");
    assert!(
        !guest.log()?.contains("mount -o defaults"),
        "{}",
        guest.log()?
    );
    Ok(())
}

#[test]
fn mount_refuses_a_wrong_source_without_chowning() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("signature", "ext4\n")?;
    guest.set("mount-source-override", "/dev/sdc\n")?;
    assert_eq!(mount_case(&guest)?, "wrong-source");
    let log = guest.log()?;
    assert!(!log.contains("chown"), "{log}");
    Ok(())
}

#[test]
fn mount_refuses_the_root_disk_without_mounting() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("scsi", &serial_line("sda", VOLUME_ID))?;
    guest.set("signature", "ext4\n")?;
    assert_eq!(mount_case(&guest)?, "root-disk");
    let log = guest.log()?;
    assert!(!log.contains("mount -o defaults"), "{log}");
    assert!(!log.contains("chown"), "{log}");
    Ok(())
}

#[test]
fn unmount_flushes_and_unmounts() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("mount-source", "/dev/sdb\n")?;
    assert_eq!(status_of(&unmount_script(MOUNT_PATH), &guest)?, "unmounted");
    let log = guest.log()?;
    assert!(log.contains("sync"), "{log}");
    assert!(log.contains("umount /mriya"), "{log}");
    Ok(())
}

#[test]
fn unmount_reports_a_failure() -> Result<(), String> {
    let guest = Guest::new()?;
    guest.set("mount-source", "/dev/sdb\n")?;
    guest.set("umount-exit", "1\n")?;
    assert_eq!(
        status_of(&unmount_script(MOUNT_PATH), &guest)?,
        "unmount-failed"
    );
    Ok(())
}
