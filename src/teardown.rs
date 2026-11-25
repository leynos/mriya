//! Shared wording for errors that occur while tearing an instance down.
//!
//! Both the run and init workflows destroy an instance and report the
//! original failure. Appending the teardown outcome the same way in both
//! places keeps those messages consistent, and keeps the wording in one
//! place rather than duplicated per workflow.

use std::fmt::Display;

/// Appends `(teardown also failed: ...)` to `message` when `teardown_error`
/// is present, and otherwise returns `message` unchanged.
///
/// For example, `"ssh timeout"` with `Some("network down")` becomes
/// `"ssh timeout (teardown also failed: network down)"`, while `None` leaves
/// it as `"ssh timeout"`.
pub(crate) fn append_teardown_note<E: Display>(
    message: String,
    teardown_error: Option<&E>,
) -> String {
    if let Some(teardown) = teardown_error {
        format!("{message} (teardown also failed: {teardown})")
    } else {
        message
    }
}
