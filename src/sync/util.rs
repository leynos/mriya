//! Path manipulation and shell operation utilities.

use mockable::{DefaultEnv, Env};

/// Expands a leading `~/` prefix to the user's home directory.
///
/// If the `HOME` environment variable is not set, the function returns the
/// input string unchanged (i.e., the leading `~` is not expanded). Callers
/// should handle this case if they need a different fallback, for example
/// returning an error or using a platform-specific home directory lookup.
///
/// This reads the process environment; use [`expand_tilde_with_env`] to supply
/// an explicit environment.
///
/// # Examples
///
/// ```
/// # use mriya::sync::expand_tilde;
/// assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
/// ```
#[must_use]
pub fn expand_tilde(path: &str) -> String { expand_tilde_with_env(path, &DefaultEnv) }

/// Expands a leading `~/` prefix using `HOME` as reported by `env`.
///
/// Behaves like [`expand_tilde`], but reads `HOME` from the supplied
/// environment so callers and tests need not touch process state.
///
/// # Examples
///
/// ```
/// # use mriya::sync::expand_tilde_with_env;
/// let mut env = mockable::MockEnv::new();
/// env.expect_string()
///     .returning(|_| Some(String::from("/home/me")));
/// assert_eq!(
///     expand_tilde_with_env("~/.ssh/id_ed25519", &env),
///     "/home/me/.ssh/id_ed25519"
/// );
/// ```
#[must_use]
pub fn expand_tilde_with_env(path: &str, env: &impl Env) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = env.string("HOME")
    {
        return format!("{home}/{rest}");
    }
    path.to_owned()
}
