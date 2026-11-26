//! Executes the generated cache-device scripts against fake guest utilities.
//!
//! The scripts decide what to format and mount, so text assertions are not
//! enough: this suite runs them under a restricted `PATH` where `lsblk`,
//! `findmnt`, `blkid`, `udevadm`, `sudo`, `mount`, `umount`, `chown`, `mkdir`,
//! `mkfs.ext4`, `sync` and `sleep` are fakes that log every call. The log is
//! the evidence that a refusal never ran a destructive command.

#[path = "cache_device_support/sandbox.rs"]
mod sandbox;

use anyhow::{Result, ensure};
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
    fn new() -> Result<Self> {
        let sandbox = Sandbox::new().map_err(anyhow::Error::msg)?;
        sandbox
            .link_tools(&["bash", "awk"])
            .map_err(anyhow::Error::msg)?;
        for (name, contents) in [
            ("root-source", "/dev/sda1\n"),
            ("pkname-sda1", "sda\n"),
            ("scsi", &serial_line("sdb", VOLUME_ID)),
            ("mkfs-exit", "0\n"),
            ("umount-exit", "0\n"),
        ] {
            sandbox
                .write_file(name, contents.as_bytes())
                .map_err(anyhow::Error::msg)?;
        }

        for (name, body) in fakes(sandbox.root()) {
            sandbox.write_tool(name, body).map_err(anyhow::Error::msg)?;
        }
        Ok(Self { sandbox })
    }

    fn set(&self, name: &str, contents: &str) -> Result<()> {
        self.sandbox
            .write_file(name, contents.as_bytes())
            .map_err(anyhow::Error::msg)
    }

    fn clear(&self, name: &str) -> Result<()> { self.set(name, "") }

    fn run(&self, script: &str) -> Result<(String, Option<i32>)> {
        let mut command = self.sandbox.command().map_err(anyhow::Error::msg)?;
        command.arg("-c").arg(script);
        let output = command
            .output()
            .map_err(|error| anyhow::anyhow!("running the script: {error}"))?;
        Ok((
            String::from_utf8_lossy(&output.stdout).into_owned(),
            output.status.code(),
        ))
    }

    fn log(&self) -> Result<String> {
        Ok(self
            .sandbox
            .read("tools.log")
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default())
    }
}

/// Renders one `lsblk -P` record for a disk.
fn serial_line(name: &str, volume_id: &str) -> String {
    format!(
        "NAME=\"{name}\" VENDOR=\"SCW\" MODEL=\"sbs\" SERIAL=\"volume-{volume_id}\" TYPE=\"disk\"\n"
    )
}

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
fn fake_blkid(root: &str) -> String {
    format!(
        concat!(
            "printf 'blkid %s\\n' \"$*\" >> '{root}/tools.log'\n",
            "if [ -f '{root}/signature' ]; then {signature}; else exit 2; fi"
        ),
        root = root,
        signature = read_state(root, "signature"),
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
fn fakes(root: &str) -> Vec<(&'static str, String)> {
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

/// Runs a script and requires the parsed status.
fn status_of(script: &str, guest: &Guest) -> Result<String> {
    let (stdout, code) = guest.run(script)?;
    ensure!(code == Some(0), "script exited {code:?}: {stdout}");
    let report = parse_report(&stdout).map_err(|error| anyhow::anyhow!("{error}: {stdout}"))?;
    Ok(report.status().as_str().to_owned())
}

fn format_case(guest: &Guest) -> Result<String> { status_of(&format_script(VOLUME_ID), guest) }

fn mount_case(guest: &Guest) -> Result<String> {
    status_of(&mount_script(VOLUME_ID, MOUNT_PATH, "ubuntu"), guest)
}

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
    guest.set("signature", "ext4\n")?;
    ensure!(format_case(&guest)? == "signature-present");
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
    guest.set("signature", "ext4\n")?;
    ensure!(mount_case(&guest)? == "verified");
    let log = guest.log()?;
    ensure!(log.contains("mount -o defaults /dev/sdb /mriya"), "{log}");
    ensure!(log.contains("chown ubuntu:ubuntu /mriya"), "{log}");
    Ok(())
}

#[test]
fn mount_refuses_a_non_ext4_source() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature", "xfs\n")?;
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
    guest.set("signature", "ext4\n")?;
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
    guest.set("signature", "ext4\n")?;
    guest.set("mount-source-override", "/dev/sdc\n")?;
    ensure!(mount_case(&guest)? == "wrong-source");
    ensure!(!guest.log()?.contains("chown"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn mount_refuses_when_ownership_fails() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("signature", "ext4\n")?;
    guest.set("chown-exit", "1\n")?;
    ensure!(mount_case(&guest)? == "ownership-failed");
    ensure!(guest.log()?.contains("chown"), "{}", guest.log()?);
    Ok(())
}

#[test]
fn mount_refuses_the_root_disk_without_mounting() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("scsi", &serial_line("sda", VOLUME_ID))?;
    guest.set("signature", "ext4\n")?;
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
    ensure!(status_of(&unmount_script(MOUNT_PATH), &guest)? == "unmounted");
    let log = guest.log()?;
    ensure!(log.contains("sync"), "{log}");
    ensure!(log.contains("umount /mriya"), "{log}");
    Ok(())
}

#[test]
fn unmount_reports_a_failure() -> Result<()> {
    let guest = Guest::new()?;
    guest.set("mount-source", "/dev/sdb\n")?;
    guest.set("umount-exit", "1\n")?;
    ensure!(status_of(&unmount_script(MOUNT_PATH), &guest)? == "unmount-failed");
    Ok(())
}
