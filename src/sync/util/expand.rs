//! Tilde expansion and the home-directory lookup that feeds it.
//!
//! Every caller that expands a leading `~/` needs the user's home directory
//! from somewhere. Keeping the expansion here, and taking `home` as an
//! argument, means the expansion itself is free of I/O and testable directly;
//! the one environment lookup lives in [`home_from_env`], which reports why
//! `HOME` could not be read rather than silently leaving the path alone.

use mockable::Env;
use thiserror::Error;

/// Raised when a `~/` path needs a `HOME` that is not valid UTF-8.
///
/// This carries no path because two boundaries consume it and each knows the
/// path to name: the SSH option builder adds the identity file, and cloud-init
/// adds the `--cloud-init-file` argument.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Error)]
#[error("HOME is set to a value that is not valid UTF-8")]
pub struct HomeUnavailable;

/// Expands a leading `~/` prefix in `path` to `home`.
///
/// Returns `path` unchanged when it does not start with `~/`, or when `home`
/// does not name a directory. A `~` elsewhere in the path is never expanded,
/// which keeps the rule the same as a shell's leading-tilde rule.
///
/// # Examples
///
/// ```
/// # use mriya::sync::expand_tilde_with_home;
/// assert_eq!(
///     expand_tilde_with_home("~/.ssh/id_ed25519", Some("/home/me")),
///     "/home/me/.ssh/id_ed25519"
/// );
/// assert_eq!(expand_tilde_with_home("/absolute", Some("/home/me")), "/absolute");
/// assert_eq!(expand_tilde_with_home("~/.ssh/id_ed25519", None), "~/.ssh/id_ed25519");
/// ```
#[must_use]
pub fn expand_tilde_with_home(path: &str, home: Option<&str>) -> String {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(directory)) => format!("{directory}/{rest}"),
        _ => path.to_owned(),
    }
}

/// Reads the home directory a `~/` path in `path` should expand to.
///
/// A path without the `~/` prefix needs no home directory, so this returns
/// `Ok(None)` for it without reading the environment at all. An unset `HOME`
/// also yields `Ok(None)`, leaving the caller free to apply a fallback such as
/// the platform's user database. A `HOME` that is set but cannot be decoded
/// yields a [`HomeUnavailable`] error instead, because treating it as absent
/// would hand the literal path `~/…` to `ssh` or to a file read and fail for a
/// reason that does not name the real cause.
///
/// # Errors
///
/// Returns [`HomeUnavailable`] when `path` starts with `~/` and `HOME` is set
/// to a value that is not valid Unicode.
///
/// # Examples
///
/// ```
/// # use mockable::MockEnv;
/// # use mriya::sync::home_from_env;
/// let mut env = MockEnv::new();
/// env.expect_raw().returning(|_| Ok(String::from("/home/me")));
/// assert_eq!(
///     home_from_env(&env, "~/.ssh/id_ed25519").expect("HOME should be readable"),
///     Some(String::from("/home/me"))
/// );
/// // A path that needs no home directory never reads the variable.
/// assert_eq!(home_from_env(&env, "/opt/key").expect("no lookup needed"), None);
/// ```
pub fn home_from_env(env: &impl Env, path: &str) -> Result<Option<String>, HomeUnavailable> {
    if !path.starts_with("~/") {
        return Ok(None);
    }
    match env.raw("HOME") {
        Ok(home) => Ok(Some(home)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(HomeUnavailable),
    }
}
