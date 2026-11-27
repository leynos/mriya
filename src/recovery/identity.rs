//! Identity of the process that owns a recovery journal.
//!
//! A process id alone cannot prove ownership: ids are reused, so the kernel's
//! start time for that id is recorded too. Both must match for the owner to
//! count as alive.

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use serde::{Deserialize, Serialize};

/// Identity of the process that owns a journal.
///
/// A process id alone is not enough: ids are reused, so the kernel's start time
/// for that id is recorded too. Both must match for the owner to count as
/// alive. On a platform without `/proc`, the owner is not identified and the
/// liveness check is unavailable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessIdentity {
    /// Process identifier.
    pub pid: u32,
    /// Kernel start time of that process, in clock ticks since boot.
    pub start_time: u64,
}

impl ProcessIdentity {
    /// Captures the current process, or `None` where `/proc` is unavailable.
    #[must_use]
    pub fn current() -> Option<Self> {
        let pid = std::process::id();
        Some(Self {
            pid,
            start_time: process_start_time(pid)?,
        })
    }

    /// Reports whether this exact process is still running.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        process_start_time(self.pid).is_some_and(|start| start == self.start_time)
    }
}

/// Reads field 22 (`starttime`) from `/proc/<pid>/stat`.
///
/// The `comm` field can contain spaces and parentheses, so parsing starts after
/// the last `)`.
fn process_start_time(pid: u32) -> Option<u64> {
    let proc_dir = Dir::open_ambient_dir("/proc", ambient_authority()).ok()?;
    let stat = proc_dir
        .read_to_string(Utf8Path::new(&format!("{pid}/stat")))
        .ok()?;
    let (_, after_comm) = stat.rsplit_once(')')?;
    let mut fields = after_comm.split_whitespace();
    // Fields after `comm` start at state (field 3), so `starttime` is the
    // twentieth value in this iterator.
    fields.nth(19)?.parse().ok()
}
