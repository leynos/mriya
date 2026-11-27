//! A campaign-scoped exclusion, so one campaign cannot run twice at once.
//!
//! Two invocations that share a campaign identifier would each pass their own
//! pre-sweep before either allocated, then both allocate. Each enforces its own
//! plan limits, so the combined spend can exceed what the operator declared.
//!
//! The exclusion is a lock file created with `create_new`, which the operating
//! system makes atomic: exactly one process can create it. It is held for the
//! whole campaign — the sweep, every attempt, and the final flush — and removed
//! when the guard drops. A lock left behind by a killed process makes the next
//! run refuse, which is the safe direction: an operator can see the file and
//! decide, rather than the harness guessing that it is stale.

use std::io::Write as _;

use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs::OpenOptions, fs_utf8::Dir};

use super::ports::CampaignError;

/// An exclusive hold on one campaign identifier.
#[derive(Debug)]
pub struct CampaignLock {
    /// The directory holding the lock file.
    directory: Dir,
    /// The lock file's name.
    name: String,
}

impl CampaignLock {
    /// Takes the lock for `campaign_id`, or reports who holds it.
    ///
    /// # Parameters
    ///
    /// - `campaign_id`: the campaign identifier to exclude.
    ///
    /// # Returns
    ///
    /// A guard that removes the lock file when it drops.
    ///
    /// # Errors
    ///
    /// Returns [`CampaignError::CampaignAlreadyRunning`] when another process
    /// holds the lock, and [`CampaignError::InvalidPlan`] when the runtime
    /// directory cannot be used.
    pub fn acquire(campaign_id: &str) -> Result<Self, CampaignError> {
        let directory_path = runtime_directory();
        let directory =
            Dir::open_ambient_dir(&directory_path, ambient_authority()).map_err(|err| {
                CampaignError::InvalidPlan(format!("cannot open {directory_path}: {err}"))
            })?;
        // Two distinct identifiers can sanitize to the same text — `run/1` and
        // `run_1` — so the digest keeps their locks apart while identical identifiers
        // still map to one file.
        let name = format!(
            "mriya-bench-{}-{:016x}.lock",
            sanitize(campaign_id),
            digest(campaign_id)
        );
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        let file = directory
            .as_ref()
            .open_with(&name, &options)
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::AlreadyExists => CampaignError::CampaignAlreadyRunning {
                    path: directory_path.join(&name).to_string(),
                },
                _ => CampaignError::InvalidPlan(format!(
                    "cannot create {}: {err}",
                    directory_path.join(&name)
                )),
            })?;
        // The guard is built before the contents are written, so a write
        // failure removes the lock instead of leaving an orphan that blocks
        // every later attempt.
        let guard = Self { directory, name };
        writeln!(&file, "campaign={campaign_id}").map_err(|err| {
            CampaignError::InvalidPlan(format!("cannot write the campaign lock: {err}"))
        })?;
        Ok(guard)
    }
}

impl Drop for CampaignLock {
    fn drop(&mut self) { self.directory.remove_file(&self.name).ok(); }
}

/// Returns the directory a lock file lives in.
///
/// `/tmp` is used unconditionally rather than `XDG_RUNTIME_DIR`, because two
/// invocations of the same campaign can disagree about that variable — a
/// session shell sets it, a scheduled job may not — and each would then take
/// a different lock for the same campaign.
fn runtime_directory() -> Utf8PathBuf { Utf8PathBuf::from("/tmp") }

/// Returns a stable 64-bit digest of the whole identifier.
///
/// FNV-1a is used because it is deterministic across processes and versions;
/// the digest is not a security boundary, only a way to keep identifiers that
/// sanitize alike from sharing a lock file.
fn digest(campaign_id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in campaign_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Reduces a campaign identifier to characters safe in a file name.
fn sanitize(campaign_id: &str) -> String {
    campaign_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}
