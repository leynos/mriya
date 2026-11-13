//! Shared helpers for crate-internal tests.
//!
//! This module builds [`mockable::MockEnv`] instances backed by a fixed
//! variable table, so unit tests can exercise environment-driven behaviour
//! without touching the process environment. Both accessors the production
//! code uses are stubbed: `string` for ordinary lookups, and `raw` for the
//! paths that must tell an unset variable from one that cannot be decoded.

use std::collections::HashMap;

use mockable::MockEnv;

/// Builds a mock environment that answers both accessors from `pairs` and
/// reports every other variable as unset.
///
/// `string` and `raw` agree on every key, so code that distinguishes them
/// still sees one table. A table of decodable values cannot express
/// [`std::env::VarError::NotUnicode`]; use [`mock_env_with_raw`] for that.
#[must_use]
pub fn mock_env(pairs: &[(&str, &str)]) -> MockEnv {
    let table: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    let for_raw = table.clone();
    let mut env = MockEnv::new();
    env.expect_string()
        .returning(move |key| table.get(key).cloned());
    env.expect_raw().returning(move |key| {
        for_raw
            .get(key)
            .cloned()
            .ok_or(std::env::VarError::NotPresent)
    });
    env
}

/// Builds a mock environment whose accessors both answer through `responds`.
///
/// The closure is what lets a test return
/// [`std::env::VarError::NotUnicode`] for one variable, which a
/// table-backed [`mock_env`] cannot express. `string` maps an `Ok` answer
/// through and reports anything else as `None`, so code calling either
/// accessor sees one consistent answer.
#[must_use]
pub fn mock_env_with_raw(
    responds: impl Fn(&str) -> Result<String, std::env::VarError> + Send + Sync + 'static,
) -> MockEnv {
    let shared = std::sync::Arc::new(responds);
    let for_raw = std::sync::Arc::clone(&shared);
    let mut env = MockEnv::new();
    env.expect_raw().returning(move |key| for_raw(key));
    env.expect_string().returning(move |key| shared(key).ok());
    env
}
