//! Utility functions for path manipulation and shell operations.

/// Expands a leading `~/` prefix to the user's home directory.
///
/// If the `HOME` environment variable is not set, the function returns the
/// input string unchanged (i.e., the leading `~` is not expanded). Callers
/// should handle this case if they need a different fallback, for example
/// returning an error or using a platform-specific home directory lookup.
///
/// # Examples
///
/// ```
/// # use mriya::sync::expand_tilde;
/// let home = std::env::var("HOME").expect("HOME should be set");
/// assert_eq!(expand_tilde("~/.ssh/id_ed25519"), format!("{home}/.ssh/id_ed25519"));
/// assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
/// ```
#[must_use]
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return format!("{}/{rest}", home.to_string_lossy());
    }
    path.to_owned()
}
