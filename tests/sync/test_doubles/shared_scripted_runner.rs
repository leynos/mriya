//! Shared scripted command runner used by sync tests.
//!
//! Re-exports the scripted runner from the main crate so both unit and
//! integration tests share the same implementation.

pub use mriya::test_support::ScriptedRunner;
