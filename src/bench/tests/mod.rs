//! Unit tests for the offline benchmark contract.
//!
//! The fixtures live beside these tests as real JSON Lines files, so a test
//! reads the same text an operator's campaign file would contain. That keeps
//! the fixtures honest: a change to a record's serialised shape breaks the
//! fixtures, and the tests notice, rather than a struct literal silently
//! agreeing with itself.

mod billing;
mod record;
mod validate;

/// Reads a fixture from the benchmark fixtures directory.
///
/// # Panics
///
/// Panics when the fixture is missing, because a missing fixture means the test
/// suite itself is broken rather than that the code under test failed.
fn fixture(name: &str) -> String {
    let path = format!("{}/src/bench/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{path} should read: {err}"))
}
