//! Fake guest utilities for the cache-device shell tests.
//!
//! Each fake logs its arguments to a shared file and reads its state from
//! small files under the sandbox root, so a case can model a filesystem, a
//! partition table, an ambiguous probe, a device-mapper root, a duplicate
//! serial or a failing command without touching the host.

use anyhow::{Result, ensure};
use mriya::cache_device::{format_script, mount_script, parse_report};

use super::sandbox::Sandbox;

/// Volume UUID used by every case.
pub(super) const VOLUME_ID: &str = "11111111-1111-1111-1111-111111111111";
/// Mount path used by the mount and unmount cases.
pub(super) const MOUNT_PATH: &str = "/mriya";

/// A restricted `PATH` of fake guest utilities backed by state files.
pub(super) struct Guest {
    sandbox: Sandbox,
}

impl Guest {
    pub(super) fn new() -> Result<Self> {
        let guest = Self::write_base_state()?;
        for (name, body) in super::shell::fakes(guest.sandbox.root()) {
            guest.write_tool(name, body)?;
        }
        Ok(guest)
    }

    /// Creates the sandbox and its default guest state.
    fn write_base_state() -> Result<Self> {
        let sandbox = Sandbox::new().map_err(anyhow::Error::msg)?;
        sandbox
            .link_tools(&["bash", "awk"])
            .map_err(anyhow::Error::msg)?;
        let guest = Self { sandbox };
        for state in [
            StateFile::RootSource("/dev/sda1"),
            StateFile::PknameSda1("sda"),
            StateFile::Scsi(serial_line("sdb", VOLUME_ID).trim_end()),
            // A brand-new volume has no signature, so both probes exit 2.
            StateFile::SignatureType("code=2"),
            StateFile::SignaturePartitionTable("code=2"),
            StateFile::MkfsExit("0"),
            StateFile::UmountExit("0"),
        ] {
            guest.set(state)?;
        }
        Ok(guest)
    }

    /// Writes one fake utility into the sandbox's `bin`.
    fn write_tool(&self, name: &str, body: String) -> Result<()> {
        self.sandbox
            .write_tool(name, body)
            .map_err(anyhow::Error::msg)
    }

    /// Replaces one state file for the next script run.
    pub(super) fn set(&self, state: StateFile<'_>) -> Result<()> {
        self.sandbox
            .write_file(state.name(), state.contents().as_bytes())
            .map_err(anyhow::Error::msg)
    }

    /// Runs `script` through `sh` under the restricted `PATH`.
    pub(super) fn run(&self, script: &str) -> Result<(String, Option<i32>)> {
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

    pub(super) fn log(&self) -> Result<String> {
        Ok(self
            .sandbox
            .read("tools.log")
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default())
    }
}

/// One fake-utility state file and the value it should hold.
#[derive(Clone, Copy)]
pub(super) enum StateFile<'a> {
    /// The root mount source reported by `findmnt`.
    RootSource(&'a str),
    /// The parent kernel name reported for `sda1`.
    PknameSda1(&'a str),
    /// The parent kernel name reported for the whole `sda` disk.
    PknameSda(&'a str),
    /// The parent kernel name reported for the `vg-root` mapper device.
    PknameVgRoot(&'a str),
    /// The parent kernel name reported for the `dm-0` mapper device.
    PknameDm0(&'a str),
    /// The `lsblk --scsi` output listing candidate disks.
    Scsi(&'a str),
    /// The exit status the `mkfs.ext4` fake returns.
    MkfsExit(&'a str),
    /// The exit status the `umount` fake returns.
    UmountExit(&'a str),
    /// The mount source `findmnt --mountpoint` reports.
    MountSource(&'a str),
    /// The exit status the `findmnt --mountpoint` fake returns.
    FindmntExit(&'a str),
    /// The exit status the `mountpoint` fake returns.
    MountpointExit(&'a str),
    /// The exit status the `dd` fake returns.
    DdExit(&'a str),
    /// The exit status the `lsblk` disk listing returns.
    LsblkExit(&'a str),
    /// The exit status the `lsblk` parent query returns.
    PknameExit(&'a str),
    /// The exit status the `webkit` fake returns.
    WipefsExit(&'a str),
    /// The signature listing the `wipefs` fake prints.
    WipefsOutput(&'a str),
    /// The exit status the `blockdev` fake returns.
    BlockdevExit(&'a str),
    /// The size the `blockdev` fake reports, in bytes.
    BlockdevSize(&'a str),
    /// The source the `mount` fake substitutes after mounting.
    MountSourceOverride(&'a str),
    /// The `blkid -s TYPE` result, with an optional `code=` line.
    SignatureType(&'a str),
    /// The `blkid -s PTTYPE` result, with an optional `code=` line.
    SignaturePartitionTable(&'a str),
    /// The `blkid -s UUID` result, with an optional `code=` line.
    SignatureUuid(&'a str),
    /// The exit status the `chown` fake returns.
    ChownExit(&'a str),
}

impl<'a> StateFile<'a> {
    /// Maps an ordinary case-table key and value to a state file.
    ///
    /// The serial scenarios are handled by the caller because they build
    /// records rather than plain values.
    pub(super) fn from_case(name: &'a str, value: &'a str) -> Option<Self> {
        Some(match name {
            "root-source" => Self::RootSource(value),
            "pkname-sda1" => Self::PknameSda1(value),
            "pkname-sda" => Self::PknameSda(value),
            "pkname-vg-root" => Self::PknameVgRoot(value),
            "pkname-dm-0" => Self::PknameDm0(value),
            "scsi" => Self::Scsi(value),
            "mkfs-exit" => Self::MkfsExit(value),
            "umount-exit" => Self::UmountExit(value),
            "mount-source" => Self::MountSource(value),
            "findmnt-exit" => Self::FindmntExit(value),
            "mountpoint-exit" => Self::MountpointExit(value),
            "dd-exit" => Self::DdExit(value),
            "lsblk-exit" => Self::LsblkExit(value),
            "pkname-exit" => Self::PknameExit(value),
            "wipefs-exit" => Self::WipefsExit(value),
            "wipefs-output" => Self::WipefsOutput(value),
            "blockdev-exit" => Self::BlockdevExit(value),
            "blockdev-size" => Self::BlockdevSize(value),
            "mount-source-override" => Self::MountSourceOverride(value),
            "signature-TYPE" => Self::SignatureType(value),
            "signature-PTTYPE" => Self::SignaturePartitionTable(value),
            "signature-UUID" => Self::SignatureUuid(value),
            "chown-exit" => Self::ChownExit(value),
            _ => return None,
        })
    }

    /// Returns the state file name the fakes read.
    fn name(&self) -> &'static str {
        match self {
            Self::RootSource(_) => "root-source",
            Self::PknameSda1(_) => "pkname-sda1",
            Self::PknameSda(_) => "pkname-sda",
            Self::PknameVgRoot(_) => "pkname-vg-root",
            Self::PknameDm0(_) => "pkname-dm-0",
            Self::Scsi(_) => "scsi",
            Self::MkfsExit(_) => "mkfs-exit",
            Self::UmountExit(_) => "umount-exit",
            Self::MountSource(_) => "mount-source",
            Self::FindmntExit(_) => "findmnt-exit",
            Self::MountpointExit(_) => "mountpoint-exit",
            Self::DdExit(_) => "dd-exit",
            Self::LsblkExit(_) => "lsblk-exit",
            Self::PknameExit(_) => "pkname-exit",
            Self::WipefsExit(_) => "wipefs-exit",
            Self::WipefsOutput(_) => "wipefs-output",
            Self::BlockdevExit(_) => "blockdev-exit",
            Self::BlockdevSize(_) => "blockdev-size",
            Self::MountSourceOverride(_) => "mount-source-override",
            Self::SignatureType(_) => "signature-TYPE",
            Self::SignaturePartitionTable(_) => "signature-PTTYPE",
            Self::SignatureUuid(_) => "signature-UUID",
            Self::ChownExit(_) => "chown-exit",
        }
    }

    /// Returns the file contents for this state value.
    fn contents(&self) -> String {
        match self {
            Self::RootSource(value)
            | Self::PknameSda1(value)
            | Self::PknameSda(value)
            | Self::PknameVgRoot(value)
            | Self::PknameDm0(value)
            | Self::Scsi(value)
            | Self::MkfsExit(value)
            | Self::UmountExit(value)
            | Self::MountSource(value)
            | Self::FindmntExit(value)
            | Self::MountpointExit(value)
            | Self::DdExit(value)
            | Self::LsblkExit(value)
            | Self::PknameExit(value)
            | Self::WipefsExit(value)
            | Self::WipefsOutput(value)
            | Self::BlockdevExit(value)
            | Self::BlockdevSize(value)
            | Self::MountSourceOverride(value)
            | Self::SignatureType(value)
            | Self::SignaturePartitionTable(value)
            | Self::SignatureUuid(value)
            | Self::ChownExit(value) => {
                if value.ends_with('\n') {
                    (*value).to_owned()
                } else {
                    format!("{value}\n")
                }
            }
        }
    }
}

/// `lsblk` record type, which decides whether the resolver accepts it.
#[derive(Clone, Copy)]
pub(super) enum RecordType {
    /// A whole disk, which the resolver accepts.
    Disk,
    /// A partition, which the resolver ignores.
    Part,
}

impl RecordType {
    /// Returns the type spelling `lsblk` prints.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Disk => "disk",
            Self::Part => "part",
        }
    }
}

/// Renders one `lsblk -P` record for a whole disk.
pub(super) fn serial_line(name: &str, volume_id: &str) -> String {
    serial_record(name, volume_id, RecordType::Disk)
}

/// Renders one `lsblk -P` record with an explicit record type.
pub(super) fn serial_record(name: &str, volume_id: &str, record_type: RecordType) -> String {
    format!(
        "NAME=\"{name}\" VENDOR=\"SCW\" MODEL=\"sbs\" SERIAL=\"volume-{volume_id}\" TYPE=\"{}\"\n",
        record_type.as_str()
    )
}

/// Runs a script and requires the parsed status.
pub(super) fn status_of(script: &str, guest: &Guest) -> Result<String> {
    let (stdout, code) = guest.run(script)?;
    ensure!(code == Some(0), "script exited {code:?}: {stdout}");
    let report = parse_report(&stdout).map_err(|error| anyhow::anyhow!("{error}: {stdout}"))?;
    Ok(report.status().as_str().to_owned())
}

pub(super) fn format_case(guest: &Guest) -> Result<String> {
    status_of(&format_script(VOLUME_ID), guest)
}

pub(super) fn mount_case(guest: &Guest) -> Result<String> {
    status_of(&mount_script(VOLUME_ID, MOUNT_PATH, "ubuntu"), guest)
}
