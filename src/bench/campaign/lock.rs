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
        let name = format!("mriya-bench-{}.lock", sanitize(campaign_id));
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
/// `XDG_RUNTIME_DIR` is per-user and cleaned by the session; `/tmp` is the
/// fallback when it is absent.
fn runtime_directory() -> Utf8PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
        && let Ok(path) = Utf8PathBuf::from_path_buf(runtime.into())
    {
        return path;
    }
    Utf8PathBuf::from("/tmp")
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
