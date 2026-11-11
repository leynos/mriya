//! Shared helpers for crate-internal tests.
//!
//! This module builds [`mockable::MockEnv`] instances backed by a fixed
//! variable table, so unit tests can exercise environment-driven behaviour
//! without touching the process environment.

use std::collections::HashMap;

use mockable::MockEnv;

/// Builds a mock environment that answers [`mockable::Env::string`] from
/// `pairs` and reports every other variable as unset.
#[must_use]
pub fn mock_env(pairs: &[(&str, &str)]) -> MockEnv {
    let table: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    let mut env = MockEnv::new();
    env.expect_string()
        .returning(move |key| table.get(key).cloned());
    env
}
