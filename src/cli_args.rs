//! Helpers that turn command-line arguments into values the run workflow uses.
//!
//! Three concerns live here: trimming an override before it reaches a request,
//! rejecting arguments that are unsafe to send over SSH, and joining the rest
//! into one remote command line. They are separate from [`crate::CliError`]
//! because each is a pure function of its arguments, with no state and no I/O,
//! so they can be tested without constructing a backend.

use shell_escape::unix::escape;

use crate::CliError;

/// Trims `value`, rejecting it when nothing remains.
///
/// For example, `" DEV1-M "` becomes `DEV1-M` and `"  "` is rejected for
/// `field`.
pub(crate) fn parse_override(field: &'static str, value: &str) -> Result<String, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::InvalidOverride {
            field,
            message: String::from("must not be empty or whitespace"),
        });
    }
    Ok(trimmed.to_owned())
}

/// Joins `args` into one shell command, quoting each argument with
/// `shell_escape` so the remote shell sees the original words.
///
/// For example, `["echo", "a b"]` becomes `echo 'a b'`.
pub(crate) fn render_remote_command(args: &[String]) -> String {
    let mut result = String::new();
    let mut first = true;

    for arg in args {
        if first {
            first = false;
        } else {
            result.push(' ');
        }

        let escaped = escape(arg.as_str().into());
        result.push_str(escaped.as_ref());
    }

    result
}

/// Rejects arguments containing ASCII control characters (0x00-0x1F and
/// 0x7F), which are not safe to send in the remote command line.
///
/// For example, `["echo", "ok"]` passes while `["echo", "a\nb"]` returns
/// [`CliError::InvalidCommand`], because a newline would let a caller inject a
/// second command into the remote shell line.
pub(crate) fn validate_command_args(args: &[String]) -> Result<(), CliError> {
    for arg in args {
        if arg
            .chars()
            .any(|ch| matches!(ch, '\u{0000}'..='\u{001F}' | '\u{007F}'))
        {
            return Err(CliError::InvalidCommand(String::from(concat!(
                "command arguments must not contain control characters (ASCII ",
                "0x00-0x1F or 0x7F, e.g. newline, carriage return, tab, NUL)"
            ))));
        }
    }
    Ok(())
}
