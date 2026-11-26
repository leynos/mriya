//! Guest-side identification and handling of the cache Block Storage device.
//!
//! Device names such as `/dev/vdb` are not stable: Scaleway documents that the
//! kernel assigns `sd*` names in discovery order, while the SCSI serial
//! `volume-<uuid>` is stable. This module builds the scripts that resolve the
//! serial, guard the root disk, format only an empty volume, mount only a
//! verified ext4 source, and unmount before teardown. [`parse_report`] turns
//! the single status line into a typed outcome so callers can report exactly
//! which refusal happened.

mod parse;
mod script;

pub use parse::{CacheDeviceParseError, CacheDeviceReport, CacheDeviceStatus, parse_report};
pub use script::{CACHE_LABEL, STATUS_PREFIX, format_script, mount_script, unmount_script};
